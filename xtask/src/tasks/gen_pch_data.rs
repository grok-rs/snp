//! `cargo xtask gen-pch-data`: generate the snp-hooks-data tables from pre-commit-hooks history (09 §5, ADR-031)
//! and regenerate snp-pyparsers' CPython vectors.
//!
//! * `gen-pch-data [--check] <full clone of github.com/pre-commit/pre-commit-hooks>` reads the tags
//!   v3.0.0…v6.0.0 of the clone (bare or not) with `git` and writes `crates/snp-hooks-data/src/generated/`:
//!   per-tag manifests (verbatim `.pre-commit-hooks.yaml`), the tag → commit table (09 §5.1), per-hook
//!   `snp_hooks_data::HookClass` and epoch (09 §1, §5.2 refined by the curated [`MODULES`] table below), the
//!   `VERIFIED_BLOBS` table (every module blob of every tag and its import closure, seeded and checked against
//!   09 §5.4), the console-script → module map (`setup.cfg`), `BUILTIN_MANIFEST_YAML` (the v6.0.0 manifest
//!   blocks of the ids `repo: builtin` serves), the initial parity-gate table and `GENERATED_FROM`. The output
//!   depends only on the tags, never on the clone's `HEAD`, so CI regenerates it byte for byte; `--check` compares
//!   instead of writing and fails on drift.
//! * `gen-pch-data vectors [--py312 PYTHON] [--py313 PYTHON] [--corpora DIR]` runs
//!   `xtask/oracle/pch_vectors.py` under both host interpreters (each needs `ruamel.yaml==0.19.1`) and rewrites
//!   `crates/snp-pyparsers/tests/vectors/*.json` from the corpora in `DIR` (default `target/pch-corpora`, fetched
//!   by `xtask/oracle/pch_corpora.py`), then runs `xtask/oracle/pch_expat_tables.py` under both to rewrite
//!   `crates/snp-pyparsers/src/xml/generated/` (expat's Name classes, Python's single-byte codecs per minor).
//!
//! Owner: WP-17.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail, ensure};

use crate::workspace;

/// The supported tags, oldest first (09 §5.1; the support floor is v3.0.0, Q5).
const TAGS: [&str; 16] = [
    "v3.0.0", "v3.0.1", "v3.1.0", "v3.2.0", "v3.3.0", "v3.4.0", "v4.0.0", "v4.0.1", "v4.1.0",
    "v4.2.0", "v4.3.0", "v4.4.0", "v4.5.0", "v4.6.0", "v5.0.0", "v6.0.0",
];

/// 09 §5.1, verified against the clone (a moved tag is an error, never silently accepted).
const SEED_COMMITS: [(&str, &str); 16] = [
    ("v3.0.0", "66250ba9bf5ae48f660b7648d9feca8ed0b18e58"),
    ("v3.0.1", "0e2c0f75028c039052b637c17173833aaf5be97a"),
    ("v3.1.0", "ebc15addedad713c86ef18ae9632c88e187dd0af"),
    ("v3.2.0", "e1668fe86af3810fbca72b8653fe478e66a0afdc"),
    ("v3.3.0", "9136088a246768144165fcc3ecc3d31bb686920a"),
    ("v3.4.0", "6e2418c5521b7d606e72914dced3253f9ace1205"),
    ("v4.0.0", "9cc62e1a631dd72a054fb2dccda6fdebd99aacab"),
    ("v4.0.1", "38b88246ccc552bffaaf54259d064beeee434539"),
    ("v4.1.0", "8fe62d14e0b4d7d845a7022c5c2c3ae41bdd3f26"),
    ("v4.2.0", "db7346d375eda68a0174f2c057dd97f2fbffe030"),
    ("v4.3.0", "3298ddab3c13dd77d6ce1fc0baf97691430d84b0"),
    ("v4.4.0", "f71fa2c1f9cf5cb705f73dffe4b21f7c61470ba9"),
    ("v4.5.0", "c4a0b883114b00d8d76b479c820ce7950211c99b"),
    ("v4.6.0", "2c9f875913ee60ca25ce70243dc24d5b6415598c"),
    ("v5.0.0", "cef0300fd0fc4d2a87a85fa2093c6b283ea36f4b"),
    ("v6.0.0", "3e8a8703264a2f4a69428a0aa4dcb512790b2c8c"),
];

/// 09 §5.4: the v6.0.0 module blobs (the seed of the Tier-B table), verified against the clone.
const SEED_V600_BLOBS: [(&str, &str); 27] = [
    ("util", "d6c90ae02f7b07f10232fb6d00fb6d2bf9e8784f"),
    (
        "check_added_large_files",
        "e67416238d28bd20be4a0d0406a6ca87ded486da",
    ),
    (
        "check_case_conflict",
        "475c91c4308e5c22f108f2280fb66d83f2a48863",
    ),
    (
        "check_executables_have_shebangs",
        "707863b32f401cc50a112f003955fcad2082b034",
    ),
    ("check_json", "612111c531d9a1413cdc4d62def2a8fb915092c2"),
    (
        "check_merge_conflict",
        "54a083ee5e45af91a43f23f642ccece4dda7c0d4",
    ),
    (
        "check_shebang_scripts_are_executable",
        "937425b0843e5f42a2d4a8dfc751470e1f6850c4",
    ),
    ("check_symlinks", "be8a800e84969dce1e7cf3eb8cf45809f9174532"),
    ("check_toml", "2105b072e99cca264ace2884dd593dc1c1ab078d"),
    (
        "check_vcs_permalinks",
        "108656aad58b84386efe84ce13da2e527a0c6da8",
    ),
    ("check_xml", "ff5536b5857a17cc290c40fbc7bb017ed6112d3c"),
    ("check_yaml", "c94ea7168082d4e6f4e85464d48fdaa84a2fbe30"),
    (
        "destroyed_symlinks",
        "9bc25898aaba42f1e15bac73c3efecd74fa47824",
    ),
    (
        "detect_aws_credentials",
        "85822886e2bdfef344d77de3cc716a4011c582af",
    ),
    (
        "detect_private_key",
        "9ad703ae6a714a3f09322a270303b1d0950ff891",
    ),
    (
        "end_of_file_fixer",
        "a88425c6815d0e876ed3168ca93bd1f630b531b9",
    ),
    (
        "file_contents_sorter",
        "4435d6a61ddc03b7f7c9b6c22150029df7b55197",
    ),
    (
        "fix_byte_order_marker",
        "100ffeac338e71c8c8e59ec001e4abe5997e85ac",
    ),
    (
        "forbid_new_submodules",
        "b7a63cd25df4a9295fa6835e2b8833fd0d0e16df",
    ),
    (
        "mixed_line_ending",
        "2fbf067fc96bd11623a9d69b33af652e7c1031d9",
    ),
    (
        "no_commit_to_branch",
        "b0b8b23803222c68abbc1ea28474fe58aba73c64",
    ),
    (
        "pretty_format_json",
        "501f37f7f7a6d9384b1d81f55295d28d5d0a6c29",
    ),
    ("removed", "fb2b6d98c6c0823b7b03b69dfa52ee23b8bb6088"),
    (
        "requirements_txt_fixer",
        "8ce8ec643a233bffb209e3eb2c384314adb23a9a",
    ),
    (
        "sort_simple_yaml",
        "65e6b7a635b570d6c91227a41774b19e8bb78d1c",
    ),
    (
        "tests_should_end_in_test",
        "07af277dfd8be0c7d0f04c76b5b9bc7ca01377e6",
    ),
    (
        "trailing_whitespace_fixer",
        "dab8b14a83b50a0d6e327d95aa84e1e4fcc9a72e",
    ),
];

/// 09 §5.4: the v6.0.0 `.pre-commit-hooks.yaml` blob.
const SEED_V600_MANIFEST_BLOB: &str = "a98a1e574bb3b8d2255f7215a97d6b15e1210a2e";

/// How snp runs a module (09 §1).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Kind {
    /// NATIVE: byte-exact port.
    Native,
    /// NATIVE-WITH-CAVEATS (third-party parser diagnostics).
    Caveat,
    /// DELEGATE: needs CPython's `ast`/`tokenize`.
    Delegate,
    /// `pre-commit-hooks-removed` (the removed-hook stubs).
    Removed,
    /// Imported by the hook modules only (`util`, the package `__init__`).
    Support,
}

/// One behaviour epoch of a module: the first tag it applies to (inclusive; until the next row), its number
/// (`0` = a DELEGATE epoch that snp does not reproduce) and what distinguishes it.
type EpochRow = (&'static str, u16, &'static str);

/// The curated epoch table (09 §5.2, refined: every observable difference between two blobs of a module, CLI
/// flags and help texts included, starts a new epoch, so a blob always determines its epoch). The generator
/// fails when a blob of a module would map to two epochs or a module of a tag is missing here.
const MODULES: &[(&str, Kind, &[EpochRow])] = &[
    (
        "__init__",
        Kind::Support,
        &[(
            "v3.0.0",
            1,
            "empty package marker (imported before every module)",
        )],
    ),
    (
        "util",
        Kind::Support,
        &[("v3.0.0", 1, "cmd_output, added_files (zsplit from v3.4.0)")],
    ),
    (
        "check_added_large_files",
        Kind::Native,
        &[
            ("v3.0.0", 1, "added ∩ files − `git lfs status --json`"),
            (
                "v3.3.0",
                2,
                "`--enforce-all`; lfs via `git lfs status --json`",
            ),
            (
                "v4.1.0",
                3,
                "lfs via `git check-attr filter -z --stdin`; help typo `Maximum` fixed",
            ),
        ],
    ),
    (
        "check_ast",
        Kind::Delegate,
        &[("v3.0.0", 0, "CPython `ast` (DELEGATE)")],
    ),
    (
        "check_builtin_literals",
        Kind::Delegate,
        &[("v3.0.0", 0, "CPython `ast` (DELEGATE)")],
    ),
    (
        "check_byte_order_marker",
        Kind::Native,
        &[(
            "v3.0.0",
            1,
            "real check: `{f}: Has a byte-order marker` (removed stub from v6.0.0)",
        )],
    ),
    (
        "check_case_conflict",
        Kind::Native,
        &[
            ("v3.0.0", 1, "files only"),
            ("v4.0.0", 2, "parent directories via `os.path.dirname`"),
            (
                "v4.1.0",
                3,
                "parent directories via `split('/')` (differs from epoch 2 only on Windows)",
            ),
        ],
    ),
    (
        "check_docstring_first",
        Kind::Delegate,
        &[("v3.0.0", 0, "CPython `tokenize` (DELEGATE)")],
    ),
    (
        "check_executables_have_shebangs",
        Kind::Native,
        &[
            ("v3.0.0", 1, "no git path (shebang check only)"),
            (
                "v3.1.0",
                2,
                "win32: `git ls-files --stage` without `-z` (splitlines)",
            ),
            ("v3.2.0", 3, "win32: `git ls-files -z --stage`"),
            ("v4.1.0", 4, "message gains the `git add --chmod=-x` line"),
            (
                "v4.3.0",
                5,
                "git path chosen by `git config core.fileMode` == 'false'",
            ),
        ],
    ),
    (
        "check_json",
        Kind::Native,
        &[
            ("v3.0.0", 1, "plain `json.load` (no duplicate-key check)"),
            ("v4.0.0", 2, "`Duplicate key: …` hook"),
        ],
    ),
    (
        "check_merge_conflict",
        Kind::Native,
        &[
            ("v3.0.0", 1, "literal `.git` dir; old unstripped message"),
            ("v4.1.0", 2, "`git rev-parse --git-dir`; old message"),
            (
                "v4.2.0",
                3,
                "`=======\\r\\n` pattern; `{f}:{i}: Merge conflict string '…' found`",
            ),
        ],
    ),
    (
        "check_shebang_scripts_are_executable",
        Kind::Native,
        &[
            (
                "v4.0.0",
                1,
                "first version (v4.0.0's console script names a missing `main_reverse`: delegated there)",
            ),
            ("v4.3.0", 2, "message gains the `git add --chmod=+x` line"),
            ("v6.0.0", 3, "message typo fixed (`If it is not supposed`)"),
        ],
    ),
    (
        "check_symlinks",
        Kind::Native,
        &[("v3.0.0", 1, "one epoch")],
    ),
    (
        "check_toml",
        Kind::Caveat,
        &[
            ("v3.0.0", 0, "`toml` PyPI package (DELEGATE)"),
            ("v4.3.0", 1, "`tomllib` / `tomli`"),
        ],
    ),
    (
        "check_vcs_permalinks",
        Kind::Native,
        &[
            ("v3.0.0", 1, "fixed github.com `blob/master` regex"),
            (
                "v3.4.0",
                2,
                "`--additional-github-domain`; `blob/master` only",
            ),
            ("v4.0.0", 3, "any non-hash ref (`(?![a-fA-F0-9]{4,64}/)`)"),
        ],
    ),
    (
        "check_xml",
        Kind::Caveat,
        &[("v3.0.0", 1, "one epoch (expat)")],
    ),
    (
        "check_yaml",
        Kind::Caveat,
        &[
            (
                "v3.0.0",
                1,
                concat!("ruamel.yaml; `--unsafe` help typo `cont", "structs`"),
            ),
            ("v4.5.0", 2, "help typo fixed"),
        ],
    ),
    (
        "debug_statement_hook",
        Kind::Delegate,
        &[("v3.0.0", 0, "CPython `ast` (DELEGATE)")],
    ),
    (
        "destroyed_symlinks",
        Kind::Native,
        &[("v3.4.0", 1, "one epoch")],
    ),
    (
        "detect_aws_credentials",
        Kind::Caveat,
        &[("v3.0.0", 1, "one epoch (configparser)")],
    ),
    (
        "detect_private_key",
        Kind::Native,
        &[
            ("v3.0.0", 1, "8 patterns"),
            (
                "v4.1.0",
                2,
                "10 patterns (`ENCRYPTED PRIVATE KEY`, `OpenVPN Static key V1`)",
            ),
        ],
    ),
    (
        "end_of_file_fixer",
        Kind::Native,
        &[("v3.0.0", 1, "one epoch")],
    ),
    (
        "file_contents_sorter",
        Kind::Native,
        &[
            ("v3.0.0", 1, "no options; lines `strip(b'\\n\\r')`"),
            ("v3.3.0", 2, "`--ignore-case`"),
            ("v3.4.0", 3, "`--unique`; lines `rstrip(b'\\n\\r')`"),
            ("v4.5.0", 4, "an empty result stays empty (no `\\n`)"),
            (
                "v6.0.0",
                5,
                "`--ignore-case` / `--unique` mutually exclusive",
            ),
        ],
    ),
    (
        "fix_byte_order_marker",
        Kind::Native,
        &[("v3.3.0", 1, "one epoch")],
    ),
    (
        "fix_encoding_pragma",
        Kind::Native,
        &[
            ("v3.0.0", 1, "real fixer"),
            (
                "v4.6.0",
                2,
                "stderr deprecation warning first (removed stub from v6.0.0)",
            ),
        ],
    ),
    (
        "forbid_new_submodules",
        Kind::Native,
        &[
            ("v3.0.0", 1, "argv ignored; `--staged`"),
            ("v4.1.0", 2, "filenames; `PRE_COMMIT_FROM_REF...TO_REF`"),
        ],
    ),
    (
        "mixed_line_ending",
        Kind::Native,
        &[("v3.0.0", 1, "one epoch")],
    ),
    (
        "no_commit_to_branch",
        Kind::Native,
        &[
            ("v3.0.0", 1, "default branch `master`"),
            ("v4.0.0", 2, "default branches `master`, `main`"),
        ],
    ),
    (
        "pretty_format_json",
        Kind::Native,
        &[
            ("v3.0.0", 0, "diff via text `print` (DELEGATE)"),
            ("v4.0.0", 1, "stops at the first invalid file"),
            ("v5.0.0", 2, "continues after an invalid file"),
        ],
    ),
    (
        "removed",
        Kind::Removed,
        &[
            (
                "v3.0.0",
                1,
                "`hookid, new_hookid, url = argv` (extra args crash)",
            ),
            ("v3.1.0", 2, "`argv[:3]`"),
        ],
    ),
    (
        "requirements_txt_fixer",
        Kind::Native,
        &[
            ("v3.0.0", 0, "name = `partition(b'==')` (DELEGATE)"),
            ("v3.1.0", 1, "name up to the first comparison operator"),
            ("v4.0.0", 2, "indented comments"),
            ("v4.6.0", 3, "duplicates removed"),
            ("v5.0.0", 4, "`pkg_resources==0.0.0` removed too"),
        ],
    ),
    (
        "sort_simple_yaml",
        Kind::Native,
        &[("v3.0.0", 1, "one epoch")],
    ),
    (
        "string_fixer",
        Kind::Delegate,
        &[("v3.0.0", 0, "CPython `tokenize` (DELEGATE)")],
    ),
    (
        "tests_should_end_in_test",
        Kind::Native,
        &[
            ("v3.0.0", 1, "`--django` only; `re.match`"),
            ("v4.3.0", 2, "pattern options; `fullmatch`"),
        ],
    ),
    (
        "trailing_whitespace_fixer",
        Kind::Native,
        &[("v3.0.0", 1, "one epoch")],
    ),
];

/// The console script of the removed-hook stubs.
const REMOVED_ENTRY: &str = "pre-commit-hooks-removed";

/// Where the generated tables live (relative to the workspace root).
const OUT_DIR: &str = "crates/snp-hooks-data/src/generated";

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let root = workspace::root()?;
    let mut it = args.iter();
    match it
        .next()
        .map(|a| a.to_string_lossy().into_owned())
        .as_deref()
    {
        Some("vectors") => vectors(&root, &it.cloned().collect::<Vec<_>>()),
        Some("--check") => {
            let clone = it
                .next()
                .context("usage: gen-pch-data [--check] <pre-commit-hooks clone>")?;
            ensure!(
                it.next().is_none(),
                "usage: gen-pch-data [--check] <pre-commit-hooks clone>"
            );
            generate(&root, Path::new(clone), true)
        }
        Some(a) if !a.starts_with('-') => {
            ensure!(
                it.next().is_none(),
                "usage: gen-pch-data [--check] <pre-commit-hooks clone>"
            );
            generate(&root, Path::new(a), false)
        }
        _ => bail!(
            "usage: cargo xtask gen-pch-data [--check] <pre-commit-hooks clone>\n       \
             cargo xtask gen-pch-data vectors [--py312 PYTHON] [--py313 PYTHON] [--corpora DIR]"
        ),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// git access
// ---------------------------------------------------------------------------------------------------------------

/// `git` against the clone (bare or with a work tree), stdout bytes; fails on a non-zero exit.
fn git(clone: &Path, args: &[&str]) -> anyhow::Result<Vec<u8>> {
    let gitdir = if clone.join(".git").exists() {
        clone.join(".git")
    } else {
        clone.to_path_buf()
    };
    let out = Command::new("git")
        .arg("--git-dir")
        .arg(&gitdir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("LC_ALL", "C")
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "git {} failed ({}): {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(out.stdout)
}

fn git_text(clone: &Path, args: &[&str]) -> anyhow::Result<String> {
    String::from_utf8(git(clone, args)?)
        .with_context(|| format!("git {} printed non-UTF-8", args.join(" ")))
}

// ---------------------------------------------------------------------------------------------------------------
// the model read from the clone
// ---------------------------------------------------------------------------------------------------------------

/// One manifest hook: id, entry and language (the keys the classification needs).
#[derive(Debug, Clone)]
struct ManifestHook {
    id: String,
    entry: String,
    language: String,
}

/// Everything read for one tag.
#[derive(Debug)]
struct TagData {
    tag: &'static str,
    commit: String,
    manifest: String,
    hooks: Vec<ManifestHook>,
    /// module → blob id.
    blobs: BTreeMap<String, String>,
    /// console script → (module, function); upstream's own entry points always name `main`, except the broken
    /// `check-shebang-scripts-are-executable = …check_executables_have_shebangs:main_reverse` of v4.0.0.
    scripts: BTreeMap<String, (String, String)>,
    /// module → modules it imports from the package (direct).
    imports: BTreeMap<String, BTreeSet<String>>,
}

fn read_tag(clone: &Path, tag: &'static str) -> anyhow::Result<TagData> {
    let commit = git_text(
        clone,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{tag}^{{commit}}"),
        ],
    )?
    .trim()
    .to_owned();
    let manifest = git_text(
        clone,
        &["show", &format!("{commit}:.pre-commit-hooks.yaml")],
    )?;
    let hooks =
        parse_manifest(&manifest).with_context(|| format!("{tag}: .pre-commit-hooks.yaml"))?;
    let mut blobs = BTreeMap::new();
    for line in git_text(clone, &["ls-tree", &commit, "pre_commit_hooks/"])?.lines() {
        let (meta, path) = line
            .split_once('\t')
            .with_context(|| format!("{tag}: ls-tree line {line:?}"))?;
        let mut parts = meta.split(' ');
        let (Some(_mode), Some(kind), Some(blob)) = (parts.next(), parts.next(), parts.next())
        else {
            bail!("{tag}: ls-tree line {line:?}");
        };
        let Some(name) = path
            .strip_prefix("pre_commit_hooks/")
            .and_then(|p| p.strip_suffix(".py"))
        else {
            continue;
        };
        ensure!(kind == "blob", "{tag}: {path} is a {kind}");
        blobs.insert(name.to_owned(), blob.to_owned());
    }
    let setup = git_text(clone, &["show", &format!("{commit}:setup.cfg")])?;
    let scripts = parse_console_scripts(&setup).with_context(|| format!("{tag}: setup.cfg"))?;
    let mut imports = BTreeMap::new();
    for module in blobs.keys() {
        let src = git_text(
            clone,
            &["show", &format!("{commit}:pre_commit_hooks/{module}.py")],
        )?;
        imports.insert(module.clone(), parse_imports(&src));
    }
    Ok(TagData {
        tag,
        commit,
        manifest,
        hooks,
        blobs,
        scripts,
        imports,
    })
}

/// The hooks of a `.pre-commit-hooks.yaml` in the block style every pre-commit-hooks tag uses: `-   id: …` starts
/// a hook, indented `key: value` lines follow. Only `id`, `entry` and `language` are read; anything this reader
/// does not understand for those keys is an error (never a guess).
fn parse_manifest(text: &str) -> anyhow::Result<Vec<ManifestHook>> {
    let mut hooks: Vec<(Option<String>, Option<String>, Option<String>)> = Vec::new();
    for (raw, n) in text.lines().zip(1usize..) {
        let line = raw.trim_end();
        if line.is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let body = if let Some(rest) = line.strip_prefix('-') {
            hooks.push((None, None, None));
            rest.trim_start()
        } else {
            ensure!(
                line.starts_with(' '),
                "line {n}: unexpected top-level line {line:?}"
            );
            line.trim_start()
        };
        let Some((key, value)) = body.split_once(':') else {
            bail!("line {n}: no `key:` in {line:?}")
        };
        let Some(hook) = hooks.last_mut() else {
            bail!("line {n}: key before the first hook")
        };
        let slot = match key {
            "id" => &mut hook.0,
            "entry" => &mut hook.1,
            "language" => &mut hook.2,
            _ => continue,
        };
        ensure!(slot.is_none(), "line {n}: duplicate key {key}");
        *slot = Some(yaml_scalar(value.trim()).with_context(|| format!("line {n}: {key}"))?);
    }
    hooks
        .into_iter()
        .map(|h| match h {
            (Some(id), Some(entry), Some(language)) => Ok(ManifestHook {
                id,
                entry,
                language,
            }),
            (id, ..) => bail!("hook {id:?} lacks id, entry or language"),
        })
        .collect()
}

/// A one-line plain, single- or double-quoted YAML scalar without escapes beyond `''` (all the manifests need).
fn yaml_scalar(v: &str) -> anyhow::Result<String> {
    if let Some(inner) = v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        return Ok(inner.replace("''", "'"));
    }
    if let Some(inner) = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        ensure!(
            !inner.contains('\\'),
            "escapes in a double-quoted scalar are not supported: {v:?}"
        );
        return Ok(inner.to_owned());
    }
    ensure!(
        !v.is_empty()
            && !v.starts_with(['[', '{', '|', '>', '&', '*', '!', '%', '@', '`'])
            && !v.contains(" #"),
        "unsupported scalar {v:?}"
    );
    Ok(v.to_owned())
}

/// `[options.entry_points] console_scripts` of `setup.cfg`: `name = pre_commit_hooks.module:function`.
fn parse_console_scripts(setup: &str) -> anyhow::Result<BTreeMap<String, (String, String)>> {
    let mut out = BTreeMap::new();
    let mut section = String::new();
    let mut in_scripts = false;
    for line in setup.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            trimmed.clone_into(&mut section);
            in_scripts = false;
            continue;
        }
        if section != "[options.entry_points]" {
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            in_scripts = trimmed.split('=').next().map(str::trim) == Some("console_scripts");
            continue;
        }
        if !in_scripts || trimmed.is_empty() {
            continue;
        }
        let (name, target) = trimmed
            .split_once('=')
            .with_context(|| format!("console script {trimmed:?}"))?;
        let (module, function) = target
            .trim()
            .strip_prefix("pre_commit_hooks.")
            .and_then(|t| t.split_once(':'))
            .with_context(|| format!("console script target {target:?}"))?;
        let value = (module.to_owned(), function.to_owned());
        ensure!(
            out.insert(name.trim().to_owned(), value).is_none(),
            "duplicate script {name:?}"
        );
    }
    ensure!(!out.is_empty(), "no console_scripts");
    Ok(out)
}

/// Modules a module imports from the package: `from pre_commit_hooks.X import …`, `from pre_commit_hooks import X`,
/// `import pre_commit_hooks.X`.
fn parse_imports(src: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in src.lines() {
        let l = line.trim_start();
        if let Some(rest) = l.strip_prefix("from pre_commit_hooks.") {
            if let Some(m) = rest.split_whitespace().next() {
                out.insert(m.to_owned());
            }
        } else if let Some(rest) = l.strip_prefix("from pre_commit_hooks import ") {
            for m in rest.trim_matches(['(', ')', ' ']).split(',') {
                let m = m.split_whitespace().next().unwrap_or_default();
                if !m.is_empty() {
                    out.insert(m.to_owned());
                }
            }
        } else if let Some(m) = l
            .strip_prefix("import pre_commit_hooks.")
            .and_then(|r| r.split([' ', ',']).next())
        {
            out.insert(m.to_owned());
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// classification
// ---------------------------------------------------------------------------------------------------------------

fn module_info(module: &str) -> Option<(Kind, &'static [EpochRow])> {
    MODULES
        .iter()
        .find(|(m, ..)| *m == module)
        .map(|&(_, k, rows)| (k, rows))
}

fn tag_index(tag: &str) -> Option<usize> {
    TAGS.iter().position(|t| *t == tag)
}

/// The epoch of `module` at `tag`.
fn epoch_at(module: &str, tag: &str) -> anyhow::Result<u16> {
    let (_, rows) =
        module_info(module).with_context(|| format!("module {module} is not in MODULES"))?;
    let at = tag_index(tag).with_context(|| format!("unknown tag {tag}"))?;
    let mut found = None;
    for &(first, epoch, _) in rows {
        let i = tag_index(first).with_context(|| format!("{module}: unknown first tag {first}"))?;
        if i <= at {
            found = Some(epoch);
        }
    }
    found.with_context(|| format!("module {module} has no epoch at {tag}"))
}

/// The generated `HookClass` variant name and epoch of one manifest hook at one tag.
fn classify(t: &TagData, h: &ManifestHook) -> anyhow::Result<(&'static str, u16)> {
    if h.language == "fail" {
        return Ok(("FailLanguage", 1));
    }
    ensure!(
        h.language == "python",
        "{} {}: unexpected language {}",
        t.tag,
        h.id,
        h.language
    );
    let script = h.entry.split_whitespace().next().unwrap_or_default();
    let (module, function) = t.scripts.get(script).with_context(|| {
        format!(
            "{} {}: entry {script:?} is not a console script",
            t.tag, h.id
        )
    })?;
    ensure!(
        t.blobs.contains_key(module),
        "{} {}: module {module} missing",
        t.tag,
        h.id
    );
    if function != "main" {
        // The console script cannot even be imported (v4.0.0 `main_reverse`): Python crashes; delegate.
        return Ok(("Delegate", 0));
    }
    let (kind, _) =
        module_info(module).with_context(|| format!("module {module} is not in MODULES"))?;
    let epoch = epoch_at(module, t.tag)?;
    let class = match kind {
        Kind::Removed => {
            ensure!(
                script == REMOVED_ENTRY,
                "{}: removed module behind {script}",
                h.id
            );
            "Stub"
        }
        Kind::Support => bail!(
            "{} {}: entry maps to the support module {module}",
            t.tag,
            h.id
        ),
        _ if epoch == 0 => "Delegate",
        Kind::Delegate => bail!("{module}: a DELEGATE module with a native epoch"),
        Kind::Caveat => "NativeWithCaveats",
        Kind::Native => "Native",
    };
    Ok((class, epoch))
}

// ---------------------------------------------------------------------------------------------------------------
// generation
// ---------------------------------------------------------------------------------------------------------------

/// The generated files: path relative to [`OUT_DIR`] → contents.
type Files = BTreeMap<String, Vec<u8>>;

const HEADER: &str = "// @generated by `cargo xtask gen-pch-data` (WP-17) from pre-commit-hooks v3.0.0..v6.0.0. Do not edit.\n";

fn generate(root: &Path, clone: &Path, check: bool) -> anyhow::Result<()> {
    let mut tags = Vec::new();
    for tag in TAGS {
        tags.push(read_tag(clone, tag)?);
    }
    warn_new_tags(clone)?;
    verify_seeds(clone, &tags)?;
    let files = render(&tags)?;
    let out = root.join(OUT_DIR);
    let mut drift = Vec::new();
    for (rel, bytes) in &files {
        let path = out.join(rel);
        let current = std::fs::read(&path).ok();
        if current.as_deref() == Some(bytes.as_slice()) {
            continue;
        }
        if check {
            drift.push(rel.clone());
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
            workspace::say(&format!("wrote {OUT_DIR}/{rel}"));
        }
    }
    // Stale generated files (a tag removed from TAGS) are drift too.
    for stale in stale_files(&out, &files)? {
        if check {
            drift.push(stale);
        } else {
            std::fs::remove_file(out.join(&stale))?;
            workspace::say(&format!("removed {OUT_DIR}/{stale}"));
        }
    }
    if check {
        ensure!(
            drift.is_empty(),
            "generated tables drifted (run `cargo xtask gen-pch-data <clone>`): {drift:?}"
        );
        workspace::say(&format!("{OUT_DIR}: {} files up to date", files.len()));
    }
    Ok(())
}

fn stale_files(out: &Path, files: &Files) -> anyhow::Result<Vec<String>> {
    let dir = out.join("manifests");
    let mut stale = Vec::new();
    if dir.is_dir() {
        for e in std::fs::read_dir(&dir)? {
            let name = format!("manifests/{}", e?.file_name().to_string_lossy());
            if !files.contains_key(&name) {
                stale.push(name);
            }
        }
    }
    stale.sort();
    Ok(stale)
}

/// A newer upstream tag is reported (the data refresh adds it with its epochs); it never changes the output.
fn warn_new_tags(clone: &Path) -> anyhow::Result<()> {
    for tag in git_text(clone, &["tag", "--list", "v*"])?.lines() {
        if TAGS.contains(&tag) {
            continue;
        }
        let parse = |t: &str| -> Option<(u32, u32, u32)> {
            let mut p = t.strip_prefix('v')?.split('.').map(str::parse::<u32>);
            match (p.next(), p.next(), p.next(), p.next()) {
                (Some(Ok(a)), Some(Ok(b)), Some(Ok(c)), None) => Some((a, b, c)),
                _ => None,
            }
        };
        if parse(tag).is_some_and(|v| v > (6, 0, 0)) {
            workspace::say(&format!(
                "warning: upstream tag {tag} is not in TAGS (snp data refresh needed)"
            ));
        }
    }
    Ok(())
}

fn verify_seeds(clone: &Path, tags: &[TagData]) -> anyhow::Result<()> {
    for (t, (tag, commit)) in tags.iter().zip(SEED_COMMITS) {
        ensure!(
            t.tag == tag && t.commit == commit,
            "{tag} is {} in the clone, 09 §5.1 says {commit}",
            t.commit
        );
    }
    let v6 = tags.last().context("no tags")?;
    for (module, blob) in SEED_V600_BLOBS {
        ensure!(
            v6.blobs.get(module).map(String::as_str) == Some(blob),
            "v6.0.0 {module}.py is {:?}, 09 §5.4 says {blob}",
            v6.blobs.get(module)
        );
    }
    let manifest_blob = git_text(
        clone,
        &[
            "rev-parse",
            &format!("{}:.pre-commit-hooks.yaml", v6.commit),
        ],
    )?;
    ensure!(
        manifest_blob.trim() == SEED_V600_MANIFEST_BLOB,
        "v6.0.0 manifest blob is {}",
        manifest_blob.trim()
    );
    Ok(())
}

/// Rust string literal (debug escapes are valid Rust escapes).
fn lit(s: &str) -> String {
    format!("{s:?}")
}

fn render(tags: &[TagData]) -> anyhow::Result<Files> {
    let mut files = Files::new();

    // ---- manifests and tag profiles ----
    let mut profiles = String::new();
    writeln!(
        profiles,
        "{HEADER}//! Tier A: one profile per tag (09 §5.1, §5.3, §6.3), oldest first."
    )?;
    writeln!(profiles, "//!\n//! Owner: WP-17.\n")?;
    writeln!(profiles, "use crate::{{Epoch, HookClass, TagProfile}};\n")?;
    writeln!(profiles, "/// Every supported tag, oldest first.")?;
    writeln!(
        profiles,
        "pub(crate) static TAG_PROFILES: &[TagProfile] = &["
    )?;
    for t in tags {
        files.insert(
            format!("manifests/{}.yaml", t.tag),
            t.manifest.clone().into_bytes(),
        );
        writeln!(profiles, "    TagProfile {{")?;
        writeln!(profiles, "        tag: {},", lit(t.tag))?;
        writeln!(profiles, "        commit: {},", lit(&t.commit))?;
        writeln!(
            profiles,
            "        manifest_yaml: include_str!(\"manifests/{}.yaml\"),",
            t.tag
        )?;
        writeln!(profiles, "        hooks: &[")?;
        for h in &t.hooks {
            let (class, epoch) = classify(t, h)?;
            writeln!(
                profiles,
                "            ({}, HookClass::{class}, Epoch({epoch})),",
                lit(&h.id)
            )?;
        }
        writeln!(profiles, "        ],")?;
        writeln!(profiles, "    }},")?;
    }
    writeln!(profiles, "];")?;
    files.insert("tags.rs".to_owned(), profiles.into_bytes());

    // ---- verified blobs: every module blob of every tag (DELEGATE epochs map to Epoch(0)) ----
    let mut blobs: BTreeMap<String, (String, u16)> = BTreeMap::new();
    for t in tags {
        for (module, blob) in &t.blobs {
            let epoch = epoch_at(module, t.tag)?;
            if let Some((m, e)) = blobs.insert(blob.clone(), (module.clone(), epoch)) {
                ensure!(
                    m == *module && e == epoch,
                    "blob {blob} maps to {m}/epoch {e} and {module}/epoch {epoch} (split the epoch at {})",
                    t.tag
                );
            }
        }
    }
    let mut blob_rs = String::new();
    writeln!(
        blob_rs,
        "{HEADER}//! Tier B: `VERIFIED_BLOBS` (09 §5.4, §6.3): blob id → (module, epoch), sorted by blob id."
    )?;
    writeln!(
        blob_rs,
        "//! Every blob of `pre_commit_hooks/*.py` at every supported tag; `Epoch(0)` = a DELEGATE epoch."
    )?;
    writeln!(blob_rs, "//!\n//! Owner: WP-17.\n")?;
    writeln!(blob_rs, "use crate::Epoch;\n")?;
    writeln!(blob_rs, "/// (blob id, module, epoch), sorted by blob id.")?;
    writeln!(
        blob_rs,
        "pub(crate) static VERIFIED_BLOBS: &[(&str, &str, Epoch)] = &["
    )?;
    for (blob, (module, epoch)) in &blobs {
        writeln!(
            blob_rs,
            "    ({}, {}, Epoch({epoch})),",
            lit(blob),
            lit(module)
        )?;
    }
    writeln!(blob_rs, "];")?;
    files.insert("blobs.rs".to_owned(), blob_rs.into_bytes());

    // ---- console scripts (union over the tags; `:main` targets only; an entry never changes its module) ----
    let mut scripts: BTreeMap<String, String> = BTreeMap::new();
    for t in tags {
        for (entry, (module, function)) in &t.scripts {
            if function != "main" {
                continue;
            }
            if let Some(old) = scripts.insert(entry.clone(), module.clone()) {
                ensure!(
                    old == *module,
                    "console script {entry} maps to {old} and {module}"
                );
            }
        }
    }
    let mut scripts_rs = String::new();
    writeln!(
        scripts_rs,
        "{HEADER}//! Console scripts (`setup.cfg` `[options.entry_points]`) of every supported tag: entry → module."
    )?;
    writeln!(scripts_rs, "//!\n//! Owner: WP-17.\n")?;
    writeln!(
        scripts_rs,
        "/// (console script, module in `pre_commit_hooks/`), sorted by console script."
    )?;
    writeln!(
        scripts_rs,
        "pub(crate) static CONSOLE_SCRIPTS: &[(&str, &str)] = &["
    )?;
    for (entry, module) in &scripts {
        writeln!(scripts_rs, "    ({}, {}),", lit(entry), lit(module))?;
    }
    writeln!(scripts_rs, "];")?;
    files.insert("scripts.rs".to_owned(), scripts_rs.into_bytes());

    // ---- import closures (union over the tags; `__init__` always, the module itself never) ----
    let mut closures: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for t in tags {
        for module in t.blobs.keys() {
            let mut seen = BTreeSet::new();
            let mut todo = vec![module.clone()];
            while let Some(m) = todo.pop() {
                for dep in t.imports.get(&m).into_iter().flatten() {
                    ensure!(
                        t.blobs.contains_key(dep),
                        "{} {m}: imports missing module {dep}",
                        t.tag
                    );
                    if seen.insert(dep.clone()) {
                        todo.push(dep.clone());
                    }
                }
            }
            seen.remove(module);
            if module != "__init__" {
                seen.insert("__init__".to_owned());
            }
            closures.entry(module.clone()).or_default().extend(seen);
        }
    }
    let mut closures_rs = String::new();
    writeln!(
        closures_rs,
        "{HEADER}//! Import closures (09 §5.4): the package modules a module needs at import time, union over the"
    )?;
    writeln!(
        closures_rs,
        "//! supported tags. The package `__init__` runs before every module, so it is always part of it."
    )?;
    writeln!(closures_rs, "//!\n//! Owner: WP-17.\n")?;
    writeln!(
        closures_rs,
        "/// (module, closure without the module itself), sorted by module."
    )?;
    writeln!(
        closures_rs,
        "pub(crate) static IMPORT_CLOSURES: &[(&str, &[&str])] = &["
    )?;
    for (module, deps) in &closures {
        let list: Vec<String> = deps.iter().map(|d| lit(d)).collect();
        writeln!(
            closures_rs,
            "    ({}, &[{}]),",
            lit(module),
            list.join(", ")
        )?;
    }
    writeln!(closures_rs, "];")?;
    files.insert("closures.rs".to_owned(), closures_rs.into_bytes());

    // ---- parity gates: Exact for NATIVE ids (and stubs), Delegate for NATIVE-WITH-CAVEATS ids (ADR-031) ----
    let mut classes: BTreeMap<String, BTreeSet<&'static str>> = BTreeMap::new();
    for t in tags {
        for h in &t.hooks {
            classes
                .entry(h.id.clone())
                .or_default()
                .insert(classify(t, h)?.0);
        }
    }
    let mut gates_rs = String::new();
    writeln!(
        gates_rs,
        "// @generated by `cargo xtask gen-pch-data` (WP-17, initial table); regenerated by `cargo xtask pch-gates` (WP-27)."
    )?;
    writeln!(
        gates_rs,
        "//! CI parity-gate table (09 §9.9): (hook id, gate), sorted by id for binary search. Initial state"
    )?;
    writeln!(
        gates_rs,
        "//! (ADR-031): `Exact` for the NATIVE ids (removed stubs included), `Delegate` for the NATIVE-WITH-CAVEATS"
    )?;
    writeln!(
        gates_rs,
        "//! ids until their gate passes; ids that are never native are absent (they delegate)."
    )?;
    writeln!(gates_rs, "//!\n//! Owner: WP-17 (W1), WP-27 (W2).\n")?;
    writeln!(gates_rs, "use crate::Gate;\n")?;
    writeln!(gates_rs, "/// Sorted by hook id.")?;
    writeln!(
        gates_rs,
        "pub(crate) static PARITY_GATES: &[(&str, Gate)] = &["
    )?;
    for (id, cls) in &classes {
        let gate = if cls.contains("NativeWithCaveats") {
            "Delegate"
        } else if cls.contains("Native") || cls.contains("Stub") {
            "Exact"
        } else {
            continue;
        };
        writeln!(gates_rs, "    ({}, Gate::{gate}),", lit(id))?;
    }
    writeln!(gates_rs, "];")?;
    files.insert("parity_gates.rs".to_owned(), gates_rs.into_bytes());

    // ---- builtin manifest: the v6.0.0 blocks of the ids `repo: builtin` serves (09 §7.1, §7.2) ----
    let v6 = tags.last().context("no tags")?;
    let mut keep = BTreeSet::new();
    for h in &v6.hooks {
        if matches!(
            classify(v6, h)?.0,
            "Native" | "NativeWithCaveats" | "FailLanguage"
        ) {
            keep.insert(h.id.clone());
        }
    }
    let builtin = filter_manifest(&v6.manifest, &keep, &v6.hooks)?;
    files.insert("manifests/builtin.yaml".to_owned(), builtin.into_bytes());

    // ---- the epoch documentation table (rendered on `Epoch`) ----
    files.insert("epochs.md".to_owned(), epochs_doc(tags)?.into_bytes());

    // ---- mod.rs ----
    let mut mod_rs = String::new();
    writeln!(
        mod_rs,
        "{HEADER}//! Generated tables (`cargo xtask gen-pch-data <clone>`; CI checks them with `--check`)."
    )?;
    writeln!(
        mod_rs,
        "//!\n//! Owner: WP-17 (`parity_gates.rs`: WP-27 from W2 on).\n"
    )?;
    for module in ["blobs", "closures", "parity_gates", "scripts", "tags"] {
        writeln!(mod_rs, "pub(crate) mod {module};")?;
    }
    writeln!(mod_rs)?;
    writeln!(
        mod_rs,
        "/// `repo: builtin` manifest (09 §7.1): the pre-commit-hooks v6.0.0 manifest blocks, verbatim, of the {} ids snp",
        keep.len()
    )?;
    writeln!(
        mod_rs,
        "/// serves natively (every v6.0.0 id except the DELEGATE ids and the removed stubs)."
    )?;
    writeln!(
        mod_rs,
        "pub const BUILTIN_MANIFEST_YAML: &str = include_str!(\"manifests/builtin.yaml\");\n"
    )?;
    writeln!(
        mod_rs,
        "/// The data's provenance: the tag range and the commit of the newest tag."
    )?;
    writeln!(
        mod_rs,
        "pub const GENERATED_FROM: &str = {};",
        lit(&format!(
            "pre-commit-hooks {}..{} @ {}",
            TAGS[0], v6.tag, v6.commit
        ))
    )?;
    files.insert("mod.rs".to_owned(), mod_rs.into_bytes());
    Ok(files)
}

/// The manifest with only the hook blocks whose id is in `keep` (verbatim lines; header lines kept).
fn filter_manifest(
    text: &str,
    keep: &BTreeSet<String>,
    hooks: &[ManifestHook],
) -> anyhow::Result<String> {
    let mut out = String::new();
    let mut index = None::<usize>;
    let mut next = 0usize;
    for line in text.split_inclusive('\n') {
        if line.starts_with('-') {
            index = Some(next);
            next = next.saturating_add(1);
        }
        let include = match index {
            None => true,
            Some(i) => hooks.get(i).is_some_and(|h| keep.contains(&h.id)),
        };
        if include {
            out.push_str(line);
        }
    }
    ensure!(
        next == hooks.len(),
        "manifest blocks ({next}) != hooks ({})",
        hooks.len()
    );
    Ok(out)
}

fn epochs_doc(tags: &[TagData]) -> anyhow::Result<String> {
    let mut doc = String::new();
    writeln!(
        doc,
        "<!-- @generated by `cargo xtask gen-pch-data` (WP-17). Do not edit. -->"
    )?;
    writeln!(
        doc,
        "Behaviour epochs of the pre-commit-hooks modules (09 §5.2, refined so that a module blob always"
    )?;
    writeln!(
        doc,
        "determines its epoch). `Epoch(0)` marks a DELEGATE epoch; native epochs count from 1 per module, in"
    )?;
    writeln!(
        doc,
        "tag order. A hook's epoch is the epoch of the module its console script runs.\n"
    )?;
    writeln!(doc, "| module | epoch | tags | what changes |")?;
    writeln!(doc, "|---|---|---|---|")?;
    let last = |module: &str, from: usize, to_excl: usize| -> Option<&'static str> {
        (from..to_excl).rev().find_map(|i| {
            let t = tags.get(i)?;
            t.blobs.contains_key(module).then_some(t.tag)
        })
    };
    for &(module, _, rows) in MODULES {
        for (k, &(first, epoch, note)) in rows.iter().enumerate() {
            let from = tag_index(first).context("tag")?;
            let to = rows
                .get(k.saturating_add(1))
                .and_then(|r| tag_index(r.0))
                .unwrap_or(TAGS.len());
            let until = last(module, from, to).context("an epoch without a tag")?;
            let range = if until == first {
                first.to_owned()
            } else {
                format!("{first}–{until}")
            };
            writeln!(
                doc,
                "| `{module}` | {epoch} | {range} | {} |",
                note.replace('|', "\\|")
            )?;
        }
    }
    Ok(doc)
}

// ---------------------------------------------------------------------------------------------------------------
// vectors
// ---------------------------------------------------------------------------------------------------------------

/// `gen-pch-data vectors`: snp-pyparsers' CPython vectors from both host interpreters.
fn vectors(root: &Path, args: &[OsString]) -> anyhow::Result<()> {
    let mut py312 = PathBuf::from("/usr/bin/python3.12");
    let mut py313 = PathBuf::from("/usr/bin/python3.13");
    let mut corpora = root.join("target/pch-corpora");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let flag = a.to_string_lossy();
        let slot = match &*flag {
            "--py312" => &mut py312,
            "--py313" => &mut py313,
            "--corpora" => &mut corpora,
            _ => bail!("unknown argument {flag}"),
        };
        *slot = PathBuf::from(it.next().with_context(|| format!("{flag} needs a value"))?);
    }
    let script = root.join("xtask/oracle/pch_vectors.py");
    let out = root.join("crates/snp-pyparsers/tests/vectors");
    std::fs::create_dir_all(&out)?;
    for (python, minor) in [(&py312, "312"), (&py313, "313")] {
        let status = Command::new(python)
            .arg("-I")
            .arg(&script)
            .arg("--minor")
            .arg(minor)
            .arg("--corpora")
            .arg(&corpora)
            .arg("--out")
            .arg(&out)
            .env("PYTHONHASHSEED", "0")
            .env("PYTHONUTF8", "1")
            .status()
            .with_context(|| format!("running {}", python.display()))?;
        ensure!(
            status.success(),
            "{} {} failed ({status})",
            python.display(),
            script.display()
        );
    }
    let tables = root.join("xtask/oracle/pch_expat_tables.py");
    let gen_dir = root.join("crates/snp-pyparsers/src/xml/generated");
    for python in [&py312, &py313] {
        let status = Command::new(python)
            .arg("-I")
            .arg(&tables)
            .arg(&gen_dir)
            .env("PYTHONHASHSEED", "0")
            .env("PYTHONUTF8", "1")
            .status()
            .with_context(|| format!("running {}", python.display()))?;
        ensure!(
            status.success(),
            "{} {} failed ({status})",
            python.display(),
            tables.display()
        );
    }
    workspace::say(&format!(
        "vectors written to {}, expat tables to {}",
        workspace::rel(root, &out),
        workspace::rel(root, &gen_dir)
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_reader() {
        let m = "-   id: a\n    name: x\n    entry: 'a b'\n    language: python\n-   id: b\n    entry: \"B\"\n    language: fail\n";
        let hooks = parse_manifest(m).unwrap();
        assert_eq!(hooks.len(), 2);
        assert_eq!(
            (hooks[0].id.as_str(), hooks[0].entry.as_str()),
            ("a", "a b")
        );
        assert_eq!(
            (hooks[1].entry.as_str(), hooks[1].language.as_str()),
            ("B", "fail")
        );
        assert!(parse_manifest("-   id: [x]\n").is_err());
    }

    #[test]
    fn console_scripts_reader() {
        let cfg = "[metadata]\nname = x\n\n[options.entry_points]\nconsole_scripts =\n    a-b = pre_commit_hooks.a_b:main\n    c = pre_commit_hooks.c:main\n\n[x]\n";
        let s = parse_console_scripts(cfg).unwrap();
        assert_eq!(s.get("a-b"), Some(&("a_b".to_owned(), "main".to_owned())));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn imports_reader() {
        let src = "import os\nfrom pre_commit_hooks.util import cmd_output\nfrom pre_commit_hooks import a, b\n";
        let i = parse_imports(src);
        assert_eq!(i.into_iter().collect::<Vec<_>>(), ["a", "b", "util"]);
    }

    #[test]
    fn epochs_are_ordered_and_complete() {
        for &(module, _, rows) in MODULES {
            let mut prev = None;
            for &(first, _, _) in rows {
                let i = tag_index(first).unwrap();
                assert!(prev.is_none_or(|p| p < i), "{module}");
                prev = Some(i);
            }
        }
        assert_eq!(epoch_at("check_merge_conflict", "v4.1.0").unwrap(), 2);
        assert_eq!(epoch_at("check_toml", "v4.2.0").unwrap(), 0);
        assert!(epoch_at("fix_byte_order_marker", "v3.2.0").is_err());
    }

    #[test]
    fn builtin_filter_keeps_verbatim_blocks() {
        let m = "-   id: a\n    entry: a\n    language: python\n-   id: b\n    entry: b\n    language: python\n";
        let hooks = parse_manifest(m).unwrap();
        let keep = BTreeSet::from(["b".to_owned()]);
        assert_eq!(
            filter_manifest(m, &keep, &hooks).unwrap(),
            "-   id: b\n    entry: b\n    language: python\n"
        );
    }
}
