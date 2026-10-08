# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0] - Unreleased

Breaking release: results now match Python identify 2.6.20 exactly, tags are a
bitset, and identification of a path costs one `lstat` and at most one bounded
read. See "Migrating from 0.4" in the README.

### Added
- Tables generated from identify **2.6.20** by `cargo xtask gen-identify`, which
  parses `extensions.py` / `interpreters.py` as data (no Python is executed);
  `IDENTIFY_VERSION`, `TAG_COUNT`, `ALL_TAGS` (sorted, 323 tags = identify's
  `ALL_TAGS`).
- `Tag` (2-byte handle, `from_name`, `FromStr`, `Display`, name-ordered `Ord`, typed
  constants `Tag::FILE`, `Tag::TEXT`, …) and a `Copy` bitset `TagSet` with
  allocation-free set operations and operators, `matches_types` (pre-commit's
  `types` / `types_or` / `exclude_types` filter), sorted iteration,
  `Display` / `FromStr` (`"file, python, text"`), `from_names` reporting every unknown
  name (`UnknownTagError`).
- `TagTable` for the raw tables (`extensions::EXTENSIONS`,
  `EXTENSIONS_NEED_BINARY_CHECK`, `NAMES`, `interpreters::INTERPRETERS`): sorted
  iteration and hash lookups without allocation.
- One-pass classification API: `Meta` (`from_metadata`, `from_git_mode`,
  `from_git_mode_with`), `ExecPolicy` (`PythonCompat`, `ModeBits`), `classify` →
  `Classification::{Done, NeedsContent(Pending)}`, `Pending::{finish, finish_bytes}`,
  `tags_from_meta` (no `lstat`; no system call at all for known names),
  `tags_from_path_with`, `tags_from_metadata` (reuses the caller's `lstat` and its
  size: a small file costs open, read, close), `parse_shebang_from_file_with` (the shebang is read only if
  the file is executable under the given `ExecPolicy`, like `tags_from_path_with`).
- `FileKind::Other` for FIFOs and devices, which are never opened.
- `shlex_split` / `ShlexError` (Python `shlex.split`), `parse_shebang_bytes`,
  `is_text_bytes`, `MAX_SHEBANG_BYTES`, `TEXT_PROBE_LEN`.
- Parity tests against vectors produced by running identify 2.6.20
  (`tests/data/identify-vectors.json`, ~7000 cases incl. upstream's own test cases),
  property tests (proptest), a benchmark (`cargo bench --bench identify`).
- CI: Linux / macOS / Windows test matrix, a real MSRV job (`cargo +1.85.0 test`),
  clippy on Linux and Windows, a check that `src/generated.rs` matches the pinned
  identify wheel, and a weekly workflow that fails when identify on PyPI has drifted.

### Changed
- **Breaking:** `TagSet` is a bitset instead of `HashSet<&'static str>`;
  `contains` accepts a `Tag` or a name. `tags::TYPE_TAGS` / `MODE_TAGS` /
  `ENCODING_TAGS` are `const TagSet`s, `ALL_TAGS` is a sorted `[&str; 323]`.
- **Breaking:** the tables are renamed to identify's names (`EXTENSIONS`,
  `EXTENSIONS_NEED_BINARY_CHECK`, `NAMES`, `INTERPRETERS`) and are `TagTable`s.
- **Breaking:** `parse_shebang` / `parse_shebang_from_file` return `Vec<String>`
  (`ShebangTuple` removed); `parse_shebang` and `is_text` return `io::Result`.
- **Breaking:** `tags_from_filename` takes `impl AsRef<[u8]>` and, like identify, uses
  only the last path component (`os.path` rules of the platform, including Windows
  drives and UNC paths); non-UTF-8 names are supported.
- **Breaking:** `IdentifyError` is `#[non_exhaustive]`; `PathNotFound` holds a
  `PathBuf` and the OS error as `source`; the never-constructed `InvalidPath` and
  `InvalidUtf8` variants are removed. `FileKind` is `#[non_exhaustive]`.
- **Breaking:** on Windows every file is executable (identify parity, so unknown
  names are checked for a shebang); `ExecPolicy::ModeBits` gives git's notion (the
  owner `x` bit, `mode & 0o100`, which is what git stages as `100755`), so it agrees
  with `Meta::from_git_mode`; `from_git_mode` likewise looks only at the owner bit.
- On Unix, `executable` follows `access(2)` rules (owner / group / other bit for the
  real uid, any bit for root) instead of "any `x` bit".
- Shebangs: Python `shlex` splitting (quotes, escapes, `#` is not a comment, vertical
  tab / form feed allowed), `/usr/bin/env -S`, multi-line `nix-shell` shebangs; only
  2 bytes are read from non-scripts and at most 64 KiB from scripts (0.4 read the
  whole first line of any executable into memory).
- `parse_shebang_from_file` returns an empty command when opening *or reading* the
  file fails with `EINVAL`, as identify does; on Windows that is every Win32 error the
  C runtime reports as `EINVAL` (e.g. `ERROR_CANT_ACCESS_FILE`).
- `is_text` keeps reading until 1 KiB or end of input and retries `Interrupted`.
- `tags_from_path` opens a file at most once (shebang and encoding from one 1 KiB
  read) and never opens FIFOs or devices; `file_is_text` on a missing path reports
  `PathNotFound`.
- Lookups allocate nothing: `tags_from_filename` is ~7x faster, `tags_from_path`
  2.2x faster for known names; with a git mode, known names need no system call.
- MSRV is now 1.85 (edition 2024's minimum), verified in CI.

### Fixed
- Tables drifted from identify: 31 tags were missing from `ALL_TAGS` (e.g.
  `pyproject`, `uv-lock`, `nushell`, `xcodebuild`), 28 extensions, 3 binary-check
  extensions, 9 names and 3 interpreters were missing, 5 names had wrong tags
  (`PKGBUILD` lacked `shell`, `Gemfile.lock`, `Pipfile.lock`, `poetry.lock`,
  `meson_options.txt`), and 18 invented names (e.g. `yarn.lock` → yaml,
  `.eslintrc` → json, `package.json`) were removed.
- `..foo` no longer has the extension `foo` (`os.path.splitext`).
- The Windows build warning and Windows test compilation failures.

### Removed
- `FileIdentifier` (its custom extensions overrode identify's rules and its skip
  options produced tag sets without an encoding tag); use `classify` / `Pending`.
- `ShebangTuple`, `tags::tags_from_array`, the `phf` and `smallvec` dependencies.

## [0.4.0] - 2026-04-05

### Added
- `tags::ALL_TAGS`: union of the type, mode, encoding, extension, name and
  interpreter tags (mirrors Python's `identify.ALL_TAGS`).

### Changed
- `interpreters::INTERPRETER_TAGS` is public.

## [0.3.0] - 2026-04-05

### Added
- I/O-free identification API: `FileInfo`, `FileKind`, `tags_from_info`, `FileIdentifier::identify_from`
- Renovate configuration for automated dependency updates
- CI: MSRV check, cargo-deny audit, docs build, rust-cache, feature matrix testing

### Changed
- **Breaking**: MSRV bumped from 1.85 to 1.94
- `clap` is now optional behind the `cli` feature (`cargo install file-identify --features cli`)
- Replaced `once_cell::sync::Lazy` with `std::sync::LazyLock`
- Replaced per-call `HashSet<u8>` in `is_text()` with compile-time `const` lookup table
- Modernized code with let chains, `HashSet::from()`, `.copied()`, `.first()`
- Simplified release pipeline and config files

### Removed
- `serde` and `serde_json` from library dependencies (CLI uses manual JSON formatting)
- `once_cell` dependency (replaced by std)
- `.secrets.baseline` and `detect-secrets` pre-commit hook

## [0.2.0] - 2025-08-08

### Changed
- **Performance**: Replaced HashMap with Perfect Hash Functions (PHF) for compile-time optimization
- **Memory**: Eliminated lazy initialization overhead with static data structures  
- **Binary size**: Optimized with PHF compile-time hash generation
- **Startup**: Zero runtime hash computation for all file type lookups

### Fixed
- Improved code formatting and applied clippy suggestions for better code quality

### Technical Details
- Converted 315+ file extension mappings to PHF
- Converted 22 interpreter mappings to PHF  
- Converted 130+ special filename mappings to PHF
- Maintained 100% API compatibility and functionality
- All tests continue to pass with identical behavior

## [0.1.1] - 2025-08-08

### Changed
- **Performance**: Replaced HashMap with Perfect Hash Functions (PHF) for compile-time optimization
- **Memory**: Eliminated lazy initialization overhead with static data structures  
- **Binary size**: Optimized with PHF compile-time hash generation
- **Startup**: Zero runtime hash computation for all file type lookups

### Technical Details
- Converted 315+ file extension mappings to PHF
- Converted 22 interpreter mappings to PHF  
- Converted 130+ special filename mappings to PHF
- Maintained 100% API compatibility and functionality
- All 73 tests continue to pass with identical behavior

## [0.1.0] - 2025-08-07

### Added
- Initial Rust implementation of file identification library
- Core functionality ported from Python `identify` library
- Support for 316+ file extensions and format detection
- Shebang parsing for executable script identification
- Binary vs text content detection
- File system metadata analysis (permissions, file types)
- Command-line interface for file identification
- Comprehensive test suite with 59+ tests
- Complete API documentation with examples
- Support for Unix sockets, symlinks, and special file types

### Features
- `tags_from_path()` - comprehensive file analysis from filesystem
- `tags_from_filename()` - fast filename-only identification
- `tags_from_interpreter()` - interpreter-based script identification
- `file_is_text()` / `is_text()` - binary vs text detection
- `parse_shebang()` / `parse_shebang_from_file()` - shebang parsing
- CLI tool with JSON output and filename-only mode

### Supported File Types
- Programming languages: Python, JavaScript, Rust, Go, Java, C/C++, PHP, Ruby, Shell, etc.
- Configuration formats: JSON, YAML, TOML, XML, INI, etc.
- Documentation: Markdown, reStructuredText, AsciiDoc, etc.
- Build systems: Makefile, CMake, Bazel, Meson, etc.
- Container formats: Docker, Podman, etc.
- And 300+ more file types and extensions

[Unreleased]: https://github.com/grok-rs/file-identify/compare/v0.4.0...HEAD
[0.5.0]: https://github.com/grok-rs/file-identify/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/grok-rs/file-identify/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/grok-rs/file-identify/releases/tag/v0.3.0
[0.2.0]: https://github.com/grok-rs/file-identify/releases/tag/v0.2.0
[0.1.1]: https://github.com/grok-rs/file-identify/releases/tag/v0.1.1
[0.1.0]: https://github.com/grok-rs/file-identify/releases/tag/v0.1.0