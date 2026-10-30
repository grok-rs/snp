# D-SEC-09

Shebang lines are read at most 64 KiB deep; a longer first line counts as "no shebang".

* **Class:** SEC (COMPATIBILITY §5.3)
* **Owner:** WP-15 (`snp_proc::resolve::normalize_cmd`, through file-identify's `parse_shebang_from_file_with`,
  `MAX_SHEBANG_BYTES = 64 KiB`); the first package of the register row owns this file and
  `tests/differential/divergences/D-SEC-09.yaml` (WBS "Divergence files", ADR-071).

## Register entry (COMPATIBILITY §5.3)

| pre-commit 4.6.2 | snp |
|---|---|
| shebang reads are unbounded | shebang line capped at 64 KiB (file-identify) |

## Rationale

`parse_shebang.normalize_cmd` (`parse_shebang.py:65-85`) reads the shebang of every resolved hook executable with
identify's `parse_shebang`, which checks the first two bytes for `#!` and then calls `f.readline()` without a size
limit (`identify.py:178-219`). A hook script (or any executable found on `PATH`) whose first line is gigabytes long —
or a FIFO/device masquerading as one — makes pre-commit read it all into memory before running the hook. The same
reader classifies every extension-less executable file of the repository for `types` filtering, so a hostile file in
a pull request can exhaust memory on a reviewer's machine (ARCHITECTURE §11.4 "Shebang line: 64 KiB").

snp reads at most 64 KiB (file-identify's documented deviation, D-SEC-09 is its register entry); a longer first line is
treated as "no shebang", exactly like a first line that is not printable ASCII. No real interpreter line comes near
the bound — Linux itself executes at most 256 bytes of it (`BINPRM_BUF_SIZE`) — so the difference is observable only
with pathological files: pre-commit runs the interpreter named in the oversized line itself, snp lets the kernel
interpret the script (which truncates the line at 256 bytes). Exit statuses for every real-world script are unchanged.

## Examples

`long.sh` (mode 755) = `#!/bin/echo ` + 70 000 × `a` + `\n`; a local hook `entry: ./long.sh`, `language: system`,
`pass_filenames: false`, `verbose: true`; `run --all-files` on Linux ≥ 5.1.

pre-commit 4.6.2 (verified with `pre_commit.xargs.xargs(('./long.sh',), ())` of the 4.6.2 oracle): `normalize_cmd`
returns `('/bin/echo', 'a'×70000, './long.sh')`; the hook output is 70 011 bytes: 70 000 × `a` + ` ./long.sh\n`.

snp: no shebang is parsed, `./long.sh` is spawned directly and the kernel runs `/bin/echo` with the first 256 bytes of
the line; the hook output is 254 bytes: 243 × `a` + ` ./long.sh\n`. Both runs end `Passed`, exit 0.
