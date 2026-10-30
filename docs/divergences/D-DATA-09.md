# D-DATA-09

UTF-8 always, each line's original EOL kept, atomic replace keeping the mode

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-09.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-21 (`snp_config::migrate`, `crates/snp-config/src/migrate.rs`); the atomic, mode-keeping replace of the returned bytes is the caller's (WP-33, `snp_os::fs::atomic_replace`). Vector `CRLF migration` in `crates/snp-config/tests/vectors/migrate.json`.

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| migrate-config/autoupdate read/write with the locale encoding and rewrite CRLF files as LF; non-atomic writes | UTF-8 always, each line's original EOL kept, atomic replace keeping the mode |

## Rationale

`migrate_config` reads with universal newlines and writes with `open(config_file, 'w')`: on POSIX a CRLF file that
needs migration is rewritten with LF everywhere, on Windows every line gets CRLF; the encoding is the locale's;
and the write truncates in place, so an interrupted write leaves a truncated config (01 §10, D-CRASH-25).

snp keeps the bytes it does not migrate: the text is decoded as strict UTF-8 without newline translation, every
rewrite splices only the touched scalar (or inserts the `repos:` line with the file's own line ending), so each
line keeps its `\n` or `\r\n`. The new bytes are handed to the caller, which writes them atomically (temp file in
the same directory, fsync, rename) keeping the file's mode. When nothing needs migrating the file is untouched in
both tools.

## Examples

Input (CRLF endings):

```
-   repo: local␍␊
    hooks:␍␊
    -   id: x␍␊
        name: x␍␊
        entry: x␍␊
        language: system␍␊
        stages: [commit]␍␊
```

pre-commit 4.6.2 on Linux writes `repos:\n-   repo: local\n …        stages: [pre-commit]\n` (every ending LF).
snp writes `repos:\r\n-   repo: local\r\n …        stages: [pre-commit]\r\n` (every ending kept, the inserted
line uses the file's CRLF). Both print `Configuration has been migrated.`.
