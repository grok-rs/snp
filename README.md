# file-identify

[![Crates.io](https://img.shields.io/crates/v/file-identify.svg)](https://crates.io/crates/file-identify)
[![docs.rs](https://docs.rs/file-identify/badge.svg)](https://docs.rs/file-identify)
[![CI](https://github.com/grok-rs/file-identify/workflows/CI/badge.svg)](https://github.com/grok-rs/file-identify/actions)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.85-blue.svg)](https://blog.rust-lang.org/)

File type identification for Rust: a faithful port of Python's
[identify](https://github.com/pre-commit/identify), the library
[pre-commit](https://pre-commit.com) uses to decide which files a hook runs on
(`types`, `types_or`, `exclude_types`).

Given a path, it returns a set of tags such as `file, non-executable, python, text`,
derived from the file type, its permissions, its name or extension, its shebang and
its first kilobyte.

- **Same answers as identify.** The lookup tables are generated from identify
  **2.6.20** (`IDENTIFY_VERSION`); `ALL_TAGS` is identify's exact tag set (323 tags).
  Shebangs are split with a port of Python's `shlex`, including `/usr/bin/env -S`
  and `nix-shell` shebangs. Thousands of parity vectors produced by running identify
  itself are checked in CI on Linux, macOS and Windows.
- **Cheap.** `Tag` is 2 bytes and `TagSet` is a 48-byte `Copy` bitset; lookups never
  allocate. Identifying a path costs one `lstat` and at most one bounded read; with a
  known git index mode, known names cost no system call at all.
- **Safe on any input.** `#![forbid(unsafe_code)]`, no panics, FIFOs and devices are
  never opened, shebang reads are bounded.

## Quick start

```toml
[dependencies]
file-identify = "0.5"
```

```rust
use file_identify::{Tag, TagSet, tags_from_filename, tags_from_path};

// Name only, no I/O.
assert_eq!(tags_from_filename("Dockerfile.xenial").to_string(), "dockerfile, text");
assert_eq!(tags_from_filename("src/app.PY").to_string(), "python, text");

// Full identification: one lstat, at most one small read.
let tags = tags_from_path("Cargo.toml").expect("readable path");
assert!(tags.contains(Tag::FILE));
assert!(tags.contains("toml"));

// pre-commit's filter: types / types_or / exclude_types.
let types: TagSet = "file".parse().unwrap();
let types_or: TagSet = "rust, toml".parse().unwrap();
let exclude: TagSet = "binary".parse().unwrap();
assert!(tags.matches_types(&types, &types_or, &exclude));
```

## Tags and tag sets

```rust
use file_identify::{ALL_TAGS, IDENTIFY_VERSION, Tag, TagSet};

let python = Tag::from_name("python").expect("known tag");   // hash lookup
assert_eq!(python.as_str(), "python");
assert!(Tag::from_name("pythn").is_none());                  // what pre-commit rejects

let set = TagSet::from_names(["text", "python"]).unwrap();   // the error lists unknown names
assert_eq!(set.to_string(), "python, text");                 // Display / FromStr round-trip
assert_eq!(set | Tag::FILE, "file, python, text".parse().unwrap()); // |, &, -, ^, !
assert_eq!(ALL_TAGS.len(), 323);
println!("tags from identify {IDENTIFY_VERSION}");
```

Iteration, `Display` and `Debug` list tags in name order. `TagSet::ALL`,
`tags::TYPE_TAGS`, `tags::MODE_TAGS` and `tags::ENCODING_TAGS` are constants.

## Identification API

| Function | identify equivalent | I/O |
|---|---|---|
| `tags_from_path(path)` | `tags_from_path` | 1 `lstat`, ≤ 1 open + 1 KiB read |
| `tags_from_path_with(path, ExecPolicy)` | — | same |
| `tags_from_metadata(path, &fs::Metadata, ExecPolicy)` | — (reuse your own `lstat`) | ≤ 1 open + 1 KiB read |
| `tags_from_meta(path, Meta)` | — (kind and mode already known) | ≤ 1 open |
| `classify(Meta, name)` → `Done` / `NeedsContent(Pending)` | — | none |
| `Pending::finish(reader)`, `Pending::finish_bytes(&[u8])` | — | caller-provided |
| `tags_from_filename(name)` | `tags_from_filename` | none |
| `tags_from_interpreter(name)` | `tags_from_interpreter` | none |
| `file_is_text(path)`, `is_text(reader)`, `is_text_bytes(&[u8])` | `file_is_text`, `is_text` | ≤ 1 KiB |
| `parse_shebang_from_file(path)`, `parse_shebang(reader)`, `parse_shebang_bytes(&[u8])` | `parse_shebang_from_file`, `parse_shebang` | first line(s), bounded |
| `parse_shebang_from_file_with(path, ExecPolicy)` | — | same |
| `shlex_split(&str)` | `shlex.split` | none |

`extensions::{EXTENSIONS, EXTENSIONS_NEED_BINARY_CHECK, NAMES}` and
`interpreters::INTERPRETERS` expose the raw tables (`TagTable`: sorted iteration,
allocation-free `get`).

### Git index fast path

When you already know a file's mode from `git ls-files -s` / `git diff --raw`, skip the
`lstat` (on Unix with `core.fileMode=true` and `core.symlinks=true`; see
[Differences from identify](#differences-from-identify) for symlinks on Windows):

```rust
use file_identify::{Meta, tags_from_meta};

let meta = Meta::from_git_mode(0o100_644).expect("regular file");
// Known extension: no system call at all.
let tags = tags_from_meta("pkg/module.py", meta).unwrap();
assert_eq!(tags.to_string(), "file, non-executable, python, text");
```

When you have already called `fs::symlink_metadata` (for example to check that the
file exists, as pre-commit does with `os.path.lexists`), pass the result to
`tags_from_metadata` instead of calling `tags_from_path`: same tags, no second `lstat`,
and a file smaller than 1 KiB is read with a single `read`.

`classify` is the fully I/O-free core: it returns the final tags, or a `Pending` that
tells you whether the content is needed for the shebang and/or `text`/`binary`, and
finishes from a reader or a byte slice (virtual file systems, archives, tests).

### Executable files

`ExecPolicy::PythonCompat` (the default) reproduces `os.access(path, os.X_OK)`:
on Unix it evaluates `access(2)` rules from the mode, owner and group against the
process's real uid/gid/groups (ACLs and `noexec` mounts are not consulted); on
Windows every file is executable, exactly as identify reports it (so unknown-name
files are checked for a shebang there too). `ExecPolicy::ModeBits` uses git's notion:
the owner `x` bit only (`mode & 0o100`), exactly what `git add` records as `100755`, so
it agrees with `Meta::from_git_mode` on the staged mode (a `0o655` file is not
executable for either).

## Differences from identify

Deliberate, to stay safe on hostile input:

- FIFOs and devices are never opened (identify blocks forever on a FIFO). They are
  tagged `file`, the mode tag and their name tags, plus `binary` when the name does not
  decide the encoding.
- The shebang parser reads at most `MAX_SHEBANG_BYTES` (64 KiB); a longer first line
  counts as "no shebang" (identify reads the whole line, e.g. 50 MB of minified code).
- `os.access` is evaluated from metadata, as described above.

Platform limitations:

- On Windows a directory junction (or another name-surrogate reparse point) is tagged
  `symlink`: Rust's standard library reports it as a symlink and does not expose the
  reparse tag. identify (`os.lstat`) reports `directory` for a junction. True symlinks
  agree.

The git index fast path describes the index, not the work tree, so it agrees with
identify only where the two agree:

- `120000` gives `symlink`, but with `core.symlinks=false` (usual on Windows) git checks
  the link out as a regular file containing the target path, which identify tags
  `file`, …, `text` (and `executable` on Windows). **On Windows, or whenever
  `core.symlinks=false`, do not trust `120000`: `lstat` those entries**
  (`tags_from_path` / `tags_from_metadata`).
- `100755` / `100644` reflect the work tree only with `core.fileMode=true`.

## Keeping up with identify

`src/generated.rs` is produced by an xtask that parses identify's `extensions.py` and
`interpreters.py` **as data** (a small Python-literal evaluator; no Python code is
imported or executed):

```bash
src="$(xtask/fetch-identify.sh latest /tmp/identify)"   # wheel from PyPI, sha256-checked
cargo xtask gen-identify --src "$src"                    # rewrite src/generated.rs
cargo xtask gen-identify --src "$src" --check            # CI: fail on drift
```

The parity vectors in `tests/data/identify-vectors.json` are regenerated by running
identify itself on a trusted copy (a source checkout, or the wheel unpacked by
`fetch-identify.sh`):
`python3 -I xtask/parity/gen_vectors.py --identify "$(dirname "$src")" --out tests/data/identify-vectors.json`.

A weekly workflow (`.github/workflows/identify-sync.yml`) does both against the latest
identify on PyPI and, when the tables changed, runs the tests and opens (or updates) a
pull request on the `identify-sync` branch for review.

## CLI

```bash
cargo install file-identify --features cli

file-identify setup.py
# ["file", "non-executable", "python", "text"]
file-identify --filename-only Cargo.toml
# ["cargo", "text", "toml"]
```

Output matches Python's `identify-cli` (`json.dumps(sorted(tags))`); exit status 1 when
the path does not exist (message on stderr) or no tag was found.

## Performance

Median per call, `cargo bench --bench identify` style harness, Intel Core Ultra 7 155H,
Linux, warm page cache; 0.4.0 vs 0.5.0 in the same process:

| operation | 0.4.0 | 0.5.0 | speed-up |
|---|---|---|---|
| `tags_from_filename`, 10 mixed names (per name) | 299 ns | 43 ns | 6.9x |
| `tags_from_interpreter("/usr/bin/python3.12.1")` | 165 ns | 36 ns | 4.6x |
| types / types_or / exclude_types filter | 61 ns | 8 ns | 7.6x |
| `tags_from_path`, known extension | 1427 ns | 649 ns | 2.2x |
| `tags_from_path`, unknown extension | 2503 ns | 2060 ns | 1.2x |
| `tags_from_path`, executable script without extension | 5161 ns | 2304 ns | 2.2x |
| known extension with a git index mode (`tags_from_meta`) | 1422 ns | 44 ns | 32x |

System calls per file (strace): an executable script now costs `statx, openat, read,
close` (0.4.0: 2× `statx`, 2× `openat`, 2× `read`, 2× `close`). A 50 MB executable
without a newline after `#!` took 6400 reads and 53 MB of memory in 0.4.0; 0.5.0 stops
after 64 KiB (16 system calls, no extra memory).

## Migrating from 0.4

| 0.4 | 0.5 |
|---|---|
| `TagSet = HashSet<&'static str>` | `TagSet` bitset (`Copy`); `contains("python")` and `contains(Tag::TEXT)` both work; iterate with `iter()` (yields `Tag`) or `names()` (yields `&'static str`) |
| `tags::FILE` etc. (`&str`) | still available; typed constants are `Tag::FILE`, `Tag::TEXT`, … |
| `tags::ALL_TAGS: LazyLock<HashSet<&str>>` | `ALL_TAGS: [&str; 323]` (sorted) and `TagSet::ALL`; check names with `Tag::from_name` |
| `tags::TYPE_TAGS` / `MODE_TAGS` / `ENCODING_TAGS` (`LazyLock`) | same names, now `const TagSet` |
| `tags::tags_from_array` | `TagSet::from_names` / `iter().collect()` |
| `extensions::EXTENSION_TAGS`, `EXTENSIONS_NEED_BINARY_CHECK_TAGS`, `NAME_TAGS`, `interpreters::INTERPRETER_TAGS` (`phf::Map`) | identify's names: `EXTENSIONS`, `EXTENSIONS_NEED_BINARY_CHECK`, `NAMES`, `INTERPRETERS` (`TagTable`); the `get_*_tags` helpers remain |
| `tags_from_filename(&str)` | `tags_from_filename(impl AsRef<[u8]>)` — strips directories like identify; pass `path.as_os_str().as_encoded_bytes()` for paths |
| `parse_shebang(R) -> Result<ShebangTuple>` | `parse_shebang(R) -> io::Result<Vec<String>>` (`first()`, `len()`, indexing as on any `Vec`) |
| `parse_shebang_from_file(P) -> Result<ShebangTuple>` | `-> Result<Vec<String>>` |
| `is_text(R) -> Result<bool>` | `-> io::Result<bool>` (`IdentifyError: From<io::Error>` keeps `?` working) |
| `FileIdentifier` builder | removed: use `classify` + `Pending` (skip content by not finishing), or `tags_from_path_with` |
| `FileInfo` / `tags_from_info` | kept (`FileKind::Other` added); `classify` + `Pending::finish_bytes` is the general form |
| `FileKind { Regular, Directory, Symlink, Socket }` | `#[non_exhaustive]`, adds `Other` (FIFOs, devices) |
| `IdentifyError::{PathNotFound{path: String}, IoError, InvalidPath, InvalidUtf8}` | `#[non_exhaustive]`; `PathNotFound { path: PathBuf, source }`, `IoError { source }`; dead variants removed |
| Windows: `.exe`/`.bat`/`.cmd` are executable | every file is executable (identify parity), or `ExecPolicy::ModeBits` |

Behaviour fixes that change results: tables now equal identify 2.6.20 (31 missing tags
added, 18 invented names such as `yarn.lock` → yaml and `.eslintrc` → json removed);
quoted shebangs, `env -S` and `nix-shell` shebangs; directory prefixes in
`tags_from_filename`; `..foo` has no extension; vertical tab / form feed allowed in
shebangs; `is_text` reads a full KiB even across short reads.

## Minimum supported Rust version

Rust **1.85** (edition 2024), tested in CI with `cargo +1.85.0 test`.

## License

MIT
