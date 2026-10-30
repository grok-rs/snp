//! Catalog rows owned by WP-12 (`tests/catalog/index.tsv`, ARCHITECTURE §13.8).
//!
//! Owner: WP-12.

#[cfg(test)]
mod catalog {
    use std::ffi::OsString;

    use snp_cli::{Cli, CliOutcome};
    use snp_core::brand::Brand;
    use snp_core::env::Env;
    use snp_core::output::ColorSetting;

    fn parse(brand: Brand, argv: &[&str], env: &[(&str, &str)]) -> CliOutcome {
        let argv: Vec<OsString> = argv.iter().map(OsString::from).collect();
        snp_cli::parse(brand, &argv, &Env::from_pairs(env.iter().copied()), None)
    }

    const TOP_USAGE: &str = "usage: pre-commit [-h] [-V]\n                  {autoupdate,clean,gc,hazmat,\
init-templatedir,install,install-hooks,migrate-config,run,sample-config,try-repo,uninstall,validate-config,\
validate-manifest,help,hook-impl} ...\n";

    /// X13-SM-09 (13 X.4/X.5): argparse exit 2, re-verified — the `PRE_COMMIT_COLOR` default is converted only when
    /// `--color` is absent, and unrecognized arguments are reported by the top-level parser.
    #[test]
    fn x13_sm_09_argparse_color_env_and_top_level_unrecognized() {
        let bogus = [("PRE_COMMIT_COLOR", "bogus")];
        // `PRE_COMMIT_COLOR=bogus pre-commit sample-config --color never` → exit 0.
        match parse(
            Brand::PreCommit,
            &["sample-config", "--color", "never"],
            &bogus,
        ) {
            CliOutcome::Cli(Cli::SampleConfig(c)) => assert_eq!(c.color, ColorSetting::Never),
            other => panic!("{other:?}"),
        }
        // Without the flag: the sub-parser's usage + the conversion error, exit 2.
        match parse(Brand::PreCommit, &["sample-config"], &bogus) {
            CliOutcome::Usage(s) => assert_eq!(
                s,
                "usage: pre-commit sample-config [-h] [--color {auto,always,never}]\n\
                 pre-commit sample-config: error: argument --color: invalid use_color value: 'bogus'\n"
            ),
            other => panic!("{other:?}"),
        }
        // `pre-commit run --bogus` → the top-level usage (2 lines on CPython 3.13) + the top-level error.
        match parse(Brand::PreCommit, &["run", "--bogus"], &[]) {
            CliOutcome::Usage(s) => assert_eq!(
                s,
                format!("{TOP_USAGE}pre-commit: error: unrecognized arguments: --bogus\n")
            ),
            other => panic!("{other:?}"),
        }
        // The snp brand: same behaviour, prog `snp`.
        match parse(Brand::Snp, &["sample-config", "--color", "never"], &bogus) {
            CliOutcome::Cli(Cli::SampleConfig(c)) => assert_eq!(c.color, ColorSetting::Never),
            other => panic!("{other:?}"),
        }
        match parse(Brand::Snp, &["sample-config"], &bogus) {
            CliOutcome::Usage(s) => assert!(s.ends_with(
                "snp sample-config: error: argument --color: invalid use_color value: 'bogus'\n"
            )),
            other => panic!("{other:?}"),
        }
    }
}
