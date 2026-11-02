# D-PLAT-02

Windows: a missing absolute shebang interpreter (`#!/bin/sh`) falls back to the interpreter's basename on `PATH`.

* **Class:** PLAT (COMPATIBILITY §5.5)
* **Owner:** WP-15 (`snp_proc::resolve::normalize_cmd`); the first package of the register row owns this file and
  `tests/differential/divergences/D-PLAT-02.yaml` (WBS "Divergence files", ADR-071).
* **Tests:** `d_plat_02_basename_fallback` (`crates/snp-proc/src/resolve.rs`, the Windows rule exercised on every OS).

## Register entry (COMPATIBILITY §5.5)

| pre-commit 4.7.0 | snp |
|---|---|
| Windows: a missing absolute shebang interpreter (`#!/bin/sh`) fails the hook / crashes the legacy hook | falls back to the interpreter's basename on PATH (Git for Windows semantics) |

## Rationale

`normalize_cmd` interprets one level of shebang on every platform (`parse_shebang.py:65-85`). On Windows the
interpreter of `#!/bin/sh` or `#!/bin/bash` is the path `/bin/sh`, which `normexe` treats as a path
(`os.altsep` is `/`): `isfile('\bin\sh')` on the current drive is false, so pre-commit reports
``Executable `/bin/sh` not found`` and fails the hook (rc 1); a legacy hook with such a shebang crashes `hook-impl`
with `An unexpected error has occurred: ExecutableNotFoundError: …` (exit 3, 14 P-22/P-23). Git for Windows runs the
very same scripts by looking up the interpreter's basename on `PATH` (`compat/mingw.c:1898-1910`), and inside
`git commit` its `usr\bin` (with `sh.exe` and `bash.exe`) is on `PATH`.

snp applies Git for Windows' rule, only where pre-commit would fail: when the interpreter came from a shebang (never
for an explicit `entry:` path), starts with `/`, and is not a file, `find_executable(basename)` is tried; if that also
fails, pre-commit's message (naming the original interpreter) is kept. Every script that works under pre-commit
resolves exactly as before; scripts that fail under pre-commit now run the way `git` itself runs them. POSIX is
unaffected (the fallback is compiled in but disabled).

## Examples

Windows, Git for Windows on `PATH` (`C:\Program Files\Git\usr\bin` provides `sh.exe`), a local hook
`entry: ./hook.sh`, `language: script`, where `hook.sh` is `#!/bin/sh` + `echo ran`; `run --all-files`:

pre-commit 4.7.0 (exit 1):

```
script hook..............................................................Failed
- hook id: script-hook
- exit code: 1

Executable `/bin/sh` not found
```

snp (exit 0): `script hook…Passed` — `sh.exe` runs `./hook.sh`.
