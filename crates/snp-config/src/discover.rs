//! Which config file a command reads when `-c/--config` is not given (ADR-087, D-EXT-09, D-DIAG-06).
//!
//! pre-commit always reads `.pre-commit-config.yaml`. snp also accepts `.snp.yaml` (or `.snp.yml`) in the repository
//! root:
//!
//! * snp brand: `.snp.yaml`, then `.snp.yml`, then `.pre-commit-config.yaml`. An snp name wins over
//!   `.pre-commit-config.yaml`, with one stderr line naming the ignored file.
//! * compat brand (`pre-commit`): `.pre-commit-config.yaml` whenever it exists, so every byte stays pre-commit's;
//!   only without it is an snp name read.
//! * both snp names at once is an error (either brand, unless `.pre-commit-config.yaml` decides it in the compat
//!   brand); an explicit `-c` always wins; `SNP_COMPAT_STRICT=1` turns discovery off.
//!
//! `.pre-commit-config.yaml` keeps pre-commit's lookup (relative to the invocation cwd when it exists there, else to
//! the root: `_adjust_args_and_chdir`); an snp name is always the root's file.
//!
//! Owner: WP-21.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use snp_core::brand::Brand;
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_core::output::DiagSink;

/// pre-commit's config file (`C.CONFIG_FILE`): the `-c` default.
pub const PRE_COMMIT_CONFIG: &str = ".pre-commit-config.yaml";

/// snp's config file names, in lookup order.
pub const SNP_CONFIGS: [&str; 2] = [".snp.yaml", ".snp.yml"];

/// The local overlay names of `.pre-commit-config.yaml` (and of any `-c` file not named like an snp config).
const PRE_COMMIT_OVERLAYS: [&str; 2] = [
    ".pre-commit-config.local.yaml",
    ".pre-commit-config.local.yml",
];

/// The local overlay names of `.snp.yaml` / `.snp.yml`.
const SNP_OVERLAYS: [&str; 2] = [".snp.local.yaml", ".snp.local.yml"];

/// Set once the discovery note of this process was printed (one per run: `install --install-hooks` and the
/// commands that discover twice print it once).
static NOTED: AtomicBool = AtomicBool::new(false);

/// The outcome of [`discover`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    /// The value to use as `-c`: as given; `.pre-commit-config.yaml` (relative, pre-commit's lookup); or the root's
    /// snp config as an absolute path.
    pub config: OsString,
    /// The snp name chosen (`None`: given or `.pre-commit-config.yaml`).
    pub chosen: Option<&'static str>,
    /// `.pre-commit-config.yaml` exists but an snp name was chosen over it (snp brand).
    pub ignored: Option<&'static str>,
    /// Not given, and no candidate exists (snp brand): `config` is pre-commit's default, which is missing.
    pub missing: bool,
}

impl Discovery {
    /// `-c` as given.
    fn given(config: &OsStr) -> Self {
        Self {
            config: config.to_os_string(),
            chosen: None,
            ignored: None,
            missing: false,
        }
    }

    /// pre-commit's default.
    fn default_name(missing: bool) -> Self {
        Self {
            config: OsString::from(PRE_COMMIT_CONFIG),
            chosen: None,
            ignored: None,
            missing,
        }
    }

    /// The stderr notes of this discovery, once per process: the ignored `.pre-commit-config.yaml`, and — when
    /// `require` (the command cannot work without a config) — that no config file exists (D-DIAG-06).
    pub fn note(&self, diag: &dyn DiagSink, require: bool) {
        let text = match (self.chosen, self.ignored) {
            (Some(chosen), Some(ignored)) => Some(ignored_note(chosen, ignored)),
            _ if self.missing && require => Some(MISSING_NOTE.to_owned()),
            _ => None,
        };
        if let Some(text) = text
            && !NOTED.swap(true, Ordering::Relaxed)
        {
            diag.diag(&text);
        }
    }
}

/// D-EXT-09: the line naming the ignored file.
pub fn ignored_note(chosen: &str, ignored: &str) -> String {
    format!("snp: using {chosen}; ignoring {ignored} (pass -c/--config to choose)")
}

/// D-DIAG-06: no config file in the repository (snp brand, `-c` not given).
pub const MISSING_NOTE: &str =
    "snp: no .snp.yaml or .pre-commit-config.yaml file was found in the repository root";

/// Both snp names exist.
pub fn ambiguous_message() -> String {
    let [yaml, yml] = SNP_CONFIGS;
    format!("both {yaml} and {yml} exist; keep one, or choose with -c/--config")
}

/// The config of a command: `given` when `-c/--config` was on the command line, else the first existing candidate
/// of the brand's order (module docs) in `root`, `.pre-commit-config.yaml` looked up as pre-commit does (`cwd`, then
/// `root`).
///
/// # Errors
///
/// `FatalError` (exit 1) when both `.snp.yaml` and `.snp.yml` exist and decide the choice.
pub fn discover(
    given: Option<&OsStr>,
    cwd: &Path,
    root: &Path,
    brand: Brand,
    env: &Env,
) -> Result<Discovery, Failure> {
    if let Some(c) = given {
        return Ok(Discovery::given(c));
    }
    if env.is_truthy_flag("SNP_COMPAT_STRICT") {
        return Ok(Discovery::default_name(false));
    }
    let pre_commit = cwd.join(PRE_COMMIT_CONFIG).exists() || root.join(PRE_COMMIT_CONFIG).exists();
    if brand == Brand::PreCommit && pre_commit {
        return Ok(Discovery::default_name(false));
    }
    let found: Vec<&'static str> = SNP_CONFIGS
        .into_iter()
        .filter(|n| root.join(n).exists())
        .collect();
    match found.as_slice() {
        [] => Ok(Discovery::default_name(brand == Brand::Snp && !pre_commit)),
        [one] => Ok(Discovery {
            config: root.join(one).into_os_string(),
            chosen: Some(one),
            ignored: (brand == Brand::Snp && pre_commit).then_some(PRE_COMMIT_CONFIG),
            missing: false,
        }),
        _ => Err(Failure::fatal("FatalError", ambiguous_message())),
    }
}

/// Whether `config`'s file name is one of [`SNP_CONFIGS`].
pub fn is_snp_config(config: &Path) -> bool {
    config
        .file_name()
        .is_some_and(|n| SNP_CONFIGS.iter().any(|s| n == OsStr::new(s)))
}

/// The local overlay names next to `config`, in lookup order: `.snp.local.y[a]ml` for an snp config, else
/// `.pre-commit-config.local.y[a]ml`.
pub fn overlay_names(config: &Path) -> [&'static str; 2] {
    if is_snp_config(config) {
        SNP_OVERLAYS
    } else {
        PRE_COMMIT_OVERLAYS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Env {
        Env::from_pairs(pairs.iter().copied())
    }

    fn touch(dir: &Path, names: &[&str]) {
        for n in names {
            std::fs::write(dir.join(n), b"repos: []\n").unwrap();
        }
    }

    #[expect(
        clippy::unwrap_in_result,
        reason = "test helper: a failed fixture is a test failure"
    )]
    fn pick(names: &[&str], brand: Brand) -> Result<Discovery, Failure> {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), names);
        let mut r = discover(None, d.path(), d.path(), brand, &env(&[]));
        // the absolute path of the temporary root is not stable: keep its file name
        if let Ok(found) = &mut r
            && found.chosen.is_some()
        {
            found.config = Path::new(&found.config).file_name().unwrap().to_os_string();
        }
        r
    }

    fn name(names: &[&str], brand: Brand) -> String {
        pick(names, brand)
            .unwrap()
            .config
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn order_snp_brand() {
        assert_eq!(name(&[], Brand::Snp), ".pre-commit-config.yaml");
        assert!(pick(&[], Brand::Snp).unwrap().missing);
        assert_eq!(
            name(&[".pre-commit-config.yaml"], Brand::Snp),
            ".pre-commit-config.yaml"
        );
        assert!(
            !pick(&[".pre-commit-config.yaml"], Brand::Snp)
                .unwrap()
                .missing
        );
        assert_eq!(name(&[".snp.yml"], Brand::Snp), ".snp.yml");
        assert_eq!(name(&[".snp.yaml"], Brand::Snp), ".snp.yaml");
        let both = pick(&[".snp.yaml", ".pre-commit-config.yaml"], Brand::Snp).unwrap();
        assert_eq!(both.config, ".snp.yaml");
        assert_eq!(both.ignored, Some(".pre-commit-config.yaml"));
        let yml = pick(&[".snp.yml", ".pre-commit-config.yaml"], Brand::Snp).unwrap();
        assert_eq!(
            (yml.chosen, yml.ignored),
            (Some(".snp.yml"), Some(".pre-commit-config.yaml"))
        );
    }

    #[test]
    fn order_compat_brand() {
        let none = pick(&[], Brand::PreCommit).unwrap();
        assert_eq!(none, Discovery::default_name(false));
        let both = pick(&[".snp.yaml", ".pre-commit-config.yaml"], Brand::PreCommit).unwrap();
        assert_eq!(both, Discovery::default_name(false));
        // `.pre-commit-config.yaml` decides: the two snp names are not an error here
        let all = pick(
            &[".snp.yaml", ".snp.yml", ".pre-commit-config.yaml"],
            Brand::PreCommit,
        )
        .unwrap();
        assert_eq!(all, Discovery::default_name(false));
        let snp = pick(&[".snp.yaml"], Brand::PreCommit).unwrap();
        assert_eq!((snp.chosen, snp.ignored), (Some(".snp.yaml"), None));
    }

    #[test]
    fn two_snp_names_is_an_error() {
        for brand in [Brand::Snp, Brand::PreCommit] {
            let Err(Failure::Exc { exc, .. }) = pick(&[".snp.yaml", ".snp.yml"], brand) else {
                panic!("expected an error");
            };
            assert_eq!(&*exc.ty, "FatalError");
            assert_eq!(exc.text, ambiguous_message().into_bytes());
        }
        assert!(
            pick(
                &[".snp.yaml", ".snp.yml", ".pre-commit-config.yaml"],
                Brand::Snp
            )
            .is_err()
        );
    }

    #[test]
    fn given_and_strict_win() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), &[".snp.yaml"]);
        let r = discover(
            Some(OsStr::new("x.yaml")),
            d.path(),
            d.path(),
            Brand::Snp,
            &env(&[]),
        )
        .unwrap();
        assert_eq!(r, Discovery::given(OsStr::new("x.yaml")));
        let r = discover(
            None,
            d.path(),
            d.path(),
            Brand::Snp,
            &env(&[("SNP_COMPAT_STRICT", "1")]),
        )
        .unwrap();
        assert_eq!(r, Discovery::default_name(false));
    }

    #[test]
    fn snp_name_is_the_roots_and_cwd_counts_for_pre_commit() {
        let d = tempfile::tempdir().unwrap();
        let sub = d.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        touch(d.path(), &[".snp.yaml"]);
        touch(&sub, &[".pre-commit-config.yaml"]);
        let r = discover(None, &sub, d.path(), Brand::Snp, &env(&[])).unwrap();
        assert_eq!(Path::new(&r.config), d.path().join(".snp.yaml"));
        assert_eq!(r.ignored, Some(".pre-commit-config.yaml"));
        // compat brand: pre-commit's lookup finds the subdirectory's file
        let r = discover(None, &sub, d.path(), Brand::PreCommit, &env(&[])).unwrap();
        assert_eq!(r, Discovery::default_name(false));
    }

    #[test]
    fn overlay_pairs() {
        assert_eq!(
            overlay_names(Path::new("/r/.snp.yaml")),
            [".snp.local.yaml", ".snp.local.yml"]
        );
        assert_eq!(overlay_names(Path::new(".snp.yml")), SNP_OVERLAYS);
        assert_eq!(
            overlay_names(Path::new("/r/.pre-commit-config.yaml")),
            [
                ".pre-commit-config.local.yaml",
                ".pre-commit-config.local.yml"
            ]
        );
        assert_eq!(overlay_names(Path::new("other.yaml")), PRE_COMMIT_OVERLAYS);
    }

    #[test]
    fn note_texts() {
        assert_eq!(
            ignored_note(".snp.yaml", ".pre-commit-config.yaml"),
            "snp: using .snp.yaml; ignoring .pre-commit-config.yaml (pass -c/--config to choose)"
        );
        assert_eq!(
            ambiguous_message(),
            "both .snp.yaml and .snp.yml exist; keep one, or choose with -c/--config"
        );
    }
}
