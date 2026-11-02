# D-CRASH-18

crash (exit 3, same text) only when a selected hook's filter needs the content tier

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-22; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-18.yaml` (WBS "Divergence files", ADR-071).
* **Status:** complete (WP-22).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.7.0 | snp |
|---|---|
| an unreadable file is identified (and crashes, exit 3) even when no hook needs its contents (X13-CLS-02) | crash (exit 3, same text) only when a selected hook's filter needs the content tier |

## Rationale

pre-commit computes the complete identify tag set of every file a hook's `files`/`exclude` pass, whatever the hook's
`types` reference (`run.py:81-98`). For a regular, non-executable file whose name gives no `text`/`binary` tag,
`identify.file_is_text` opens it; when the file is unreadable (mode 000, an ACL) the `PermissionError` aborts the run
with exit 3 — even for hooks whose filter (`types: [file]`, the default) cannot depend on the content.

snp identifies in tiers (ADR-041): a filter needs the content tier only when it references `text`/`binary` or a tag a
shebang interpreter can yield. The default `types: [file]` reads no file content at all, so an unreadable file is
simply passed to the hook, which reports its own error. When a selected hook's filter does need the content, snp fails
closed with pre-commit's exact text (`An unexpected error has occurred: PermissionError: [Errno 13] Permission
denied: 'noext'`, exit 3) — a guard must not silently skip a staged file it cannot classify (X13-CLS-02).

Safe: the outcome of every filter is exactly what full identification would give (the filter outcome depends only on
the referenced tags; property-tested against full identification over the identify oracle tree), and the crash is
kept wherever the content matters.

## Examples

Recorded with pre-commit 4.7.0 on CPython 3.12.3 (a venv built from the hash lock `xtask/oracle/requirements-pc470.txt`); `<W>` is the scratch root, `<store>` the store directory.

A committed, unreadable (`chmod 000`) file `noext` and one `echo` hook with the default `types: [file]`; `run
--all-files` as a non-root user (`tests/differential/divergences/D-CRASH-18.yaml`).

pre-commit 4.7.0, exit 3:

```text
An unexpected error has occurred: PermissionError: [Errno 13] Permission denied: 'noext'
Check the log at <store>/pre-commit.log
```

snp, exit 0 (`echo` receives `noext`):

```text
echo.....................................................................Passed
```

With `types: [text]` on the hook both tools print the `PermissionError` line and exit 3.

Unit coverage: `crates/snp-files/tests/catalog.rs::x13_cls_02_unreadable_file_needed_by_a_hook_fails_with_exit_3`,
`tests/classify.rs::default_types_file_reads_no_contents_syscall_counter`.
