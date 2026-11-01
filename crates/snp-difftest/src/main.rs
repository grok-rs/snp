//! `snp-difftest`: the command-line driver of the differential harness (`cargo xtask difftest` runs it).
//!
//! ```text
//! snp-difftest run [--suite seed|curated|divergences|all|generated] [--scenario NAME]… [--file PATH]…
//!                  [--cases N] [--seed S] [--case I] [--snp-bin P] [--oracle DIR] [--cli-oracle DIR] [--root DIR]
//!                  [--out DIR]
//!                  [--brand compat|snp] [--net] [--keep] [--self-check] [--jobs N] [--no-taskset] [--no-shrink]
//!                  [--max-shrink-runs N] [--probe P] [--no-probe] [--tag T]
//! snp-difftest list [--suite …] [--tag T]
//! snp-difftest register [--strict]
//! snp-difftest generate --cases N --seed S --out DIR [--no-probe]
//! snp-difftest shrink --file PATH [runner options] [--max-shrink-runs N]
//! ```
//!
//! Exit status: 0 = every scenario passed (identical, or diverging exactly as registered, or skipped), 1 = a failure
//! (an unlisted difference, a stale divergence, a harness error, a register problem), 2 = usage error.
//!
//! Owner: WP-35.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use snp_difftest::{
    Brand, Options, Outcome, Runner, Scenario, Verdict, generate, load_scenarios, register, shrink,
};

/// Parsed command line.
#[derive(Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one field per command-line switch"
)]
struct Args {
    cmd: String,
    suite: Option<String>,
    scenarios: Vec<String>,
    files: Vec<PathBuf>,
    tags: Vec<String>,
    cases: u64,
    seed: u64,
    case: Option<u64>,
    snp_bin: Option<PathBuf>,
    oracle: Option<PathBuf>,
    cli_oracle: Option<PathBuf>,
    root: Option<PathBuf>,
    out: Option<PathBuf>,
    probe: Option<PathBuf>,
    no_probe: bool,
    brand: Vec<Brand>,
    net: bool,
    keep: bool,
    self_check: bool,
    jobs: Option<usize>,
    no_taskset: bool,
    no_shrink: bool,
    max_shrink_runs: usize,
    strict: bool,
}

fn usage(msg: &str) -> Result<Args, String> {
    Err(format!(
        "snp-difftest: {msg} (see the module documentation of crates/snp-difftest/src/main.rs)"
    ))
}

fn parse_args(argv: &[OsString]) -> Result<Args, String> {
    let mut a = Args {
        cases: 2000,
        max_shrink_runs: 400,
        ..Args::default()
    };
    let mut it = argv.iter().map(|s| s.to_string_lossy().into_owned());
    let Some(cmd) = it.next() else {
        return usage("missing command");
    };
    a.cmd = cmd;
    while let Some(arg) = it.next() {
        let mut val = |name: &str| {
            it.next()
                .ok_or_else(|| format!("snp-difftest: {name} needs a value"))
        };
        match arg.as_str() {
            "--suite" => a.suite = Some(val("--suite")?),
            "--scenario" => a.scenarios.push(val("--scenario")?),
            "--file" => a.files.push(PathBuf::from(val("--file")?)),
            "--tag" => a.tags.push(val("--tag")?),
            "--cases" => {
                a.cases = val("--cases")?
                    .parse()
                    .map_err(|_| "snp-difftest: --cases: not a number")?
            }
            "--seed" => a.seed = parse_seed(&val("--seed")?),
            "--case" => {
                a.case = Some(
                    val("--case")?
                        .parse()
                        .map_err(|_| "snp-difftest: --case: not a number")?,
                )
            }
            "--snp-bin" => a.snp_bin = Some(PathBuf::from(val("--snp-bin")?)),
            "--oracle" => a.oracle = Some(PathBuf::from(val("--oracle")?)),
            "--cli-oracle" => a.cli_oracle = Some(PathBuf::from(val("--cli-oracle")?)),
            "--root" => a.root = Some(PathBuf::from(val("--root")?)),
            "--out" => a.out = Some(PathBuf::from(val("--out")?)),
            "--probe" => a.probe = Some(PathBuf::from(val("--probe")?)),
            "--no-probe" => a.no_probe = true,
            "--brand" => a.brand.push(match val("--brand")?.as_str() {
                "compat" => Brand::Compat,
                "snp" => Brand::Snp,
                other => return usage(&format!("unknown brand {other:?}")),
            }),
            "--net" => a.net = true,
            "--keep" => a.keep = true,
            "--self-check" => a.self_check = true,
            "--jobs" | "-j" => {
                a.jobs = Some(
                    val("--jobs")?
                        .parse()
                        .map_err(|_| "snp-difftest: --jobs: not a number")?,
                );
            }
            "--no-taskset" => a.no_taskset = true,
            "--no-shrink" => a.no_shrink = true,
            "--max-shrink-runs" => {
                a.max_shrink_runs = val("--max-shrink-runs")?
                    .parse()
                    .map_err(|_| "snp-difftest: bad number")?;
            }
            "--strict" => a.strict = true,
            other => return usage(&format!("unknown argument {other:?}")),
        }
    }
    Ok(a)
}

/// A decimal seed, or the FNV hash of any other text (CI passes `$GITHUB_RUN_ID`).
fn parse_seed(s: &str) -> u64 {
    s.parse().unwrap_or_else(|_| {
        s.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, c| {
            (h ^ u64::from(c)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    })
}

fn out(s: &str) {
    let mut o = std::io::stdout().lock();
    drop(o.write_all(s.as_bytes()));
    drop(o.flush());
}

fn err(s: &str) {
    let mut o = std::io::stderr().lock();
    drop(o.write_all(s.as_bytes()));
    drop(o.flush());
}

fn main() -> ExitCode {
    let argv: Vec<OsString> = std::env::args_os().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            err(&format!("{e}\n"));
            return ExitCode::from(2);
        }
    };
    let result = match args.cmd.as_str() {
        "run" => cmd_run(&args),
        "list" => cmd_list(&args),
        "register" => cmd_register(&args),
        "generate" => cmd_generate(&args),
        "shrink" => cmd_shrink(&args),
        other => {
            err(&format!(
                "snp-difftest: unknown command {other:?} (run, list, register, generate, shrink)\n"
            ));
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            err(&format!("snp-difftest: {e}\n"));
            ExitCode::from(1)
        }
    }
}

fn workspace() -> PathBuf {
    snp_difftest::workspace_root()
}

/// The scenarios selected by `--suite`, `--file`, `--scenario` and `--tag` (not the generated suite).
fn select(a: &Args) -> Result<Vec<Scenario>, String> {
    let ws = workspace();
    let corpus = ws.join("tests/differential/scenarios");
    let divergences = ws.join("tests/differential/divergences");
    let mut all = Vec::new();
    if a.files.is_empty() || a.suite.is_some() {
        match a.suite.as_deref().unwrap_or("all") {
            "seed" => all.extend(load_scenarios(&corpus.join("seed"))?),
            "curated" => all.extend(load_scenarios(&corpus)?),
            "divergences" => all.extend(load_scenarios(&divergences)?),
            "all" => {
                all.extend(load_scenarios(&corpus)?);
                all.extend(load_scenarios(&divergences)?);
            }
            other => {
                return Err(format!(
                    "unknown suite {other:?} (seed, curated, divergences, all, generated)"
                ));
            }
        }
    }
    for f in &a.files {
        let text = std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
        all.push(Scenario::parse(&text, f)?);
    }
    if !a.scenarios.is_empty() {
        all.retain(|s| {
            a.scenarios
                .iter()
                .any(|n| s.name == *n || s.name.starts_with(&format!("{n}/")))
        });
        if all.is_empty() {
            return Err(format!("no scenario named {:?}", a.scenarios));
        }
    }
    if !a.tags.is_empty() {
        all.retain(|s| s.tags.iter().any(|t| a.tags.contains(t)));
    }
    Ok(all)
}

fn runner(a: &Args) -> Result<(Runner, Options), String> {
    let ws = workspace();
    let snp_bin = match &a.snp_bin {
        Some(p) => p.clone(),
        None => snp_testkit::Bins::locate()
            .map(|b| b.snp)
            .ok_or("no --snp-bin given and no snp binary next to snp-difftest (cargo build --release -p snp)")?,
    };
    // Runtime oracle: CPython 3.12.3; CLI-text oracle (prog-patched, snp brand): CPython 3.13 (ADR-058, 12 §A.4.1).
    let oracle = a
        .oracle
        .clone()
        .unwrap_or_else(|| ws.join("target/oracle/pc462-py312"));
    let cli_oracle = a
        .cli_oracle
        .clone()
        .unwrap_or_else(|| ws.join("target/oracle/pc462-py313"));
    let cli_oracle = if cli_oracle.exists() {
        cli_oracle
    } else {
        oracle.clone()
    };
    let bin = |name: &str| {
        if cfg!(windows) {
            oracle.join("Scripts").join(format!("{name}.exe"))
        } else {
            oracle.join("bin").join(name)
        }
    };
    let r = Runner {
        snp_bin,
        oracle_bin: bin("pre-commit"),
        oracle_snp_prog: if cfg!(windows) {
            cli_oracle.join("Scripts").join("pre-commit-snp-prog")
        } else {
            cli_oracle.join("bin").join("pre-commit-snp-prog")
        },
        root: a
            .root
            .clone()
            .unwrap_or_else(|| ws.join("target/difftest/roots")),
    };
    if !r.oracle_bin.exists() {
        return Err(format!(
            "{} not found: run `cargo xtask oracle install` (or pass --oracle)",
            r.oracle_bin.display()
        ));
    }
    let mut o = Options::detect(&r)?;
    if a.no_probe {
        o.probe = None;
    } else if let Some(p) = &a.probe {
        o.probe = Some(p.clone());
    }
    o.allow_net = a.net;
    o.keep = a.keep;
    o.self_check = a.self_check;
    o.brands.clone_from(&a.brand);
    if a.no_taskset {
        o.taskset = None;
    }
    o.artifacts = Some(
        a.out
            .clone()
            .unwrap_or_else(|| ws.join("target/difftest/out")),
    );
    Ok((r, o))
}

fn jobs(a: &Args) -> usize {
    a.jobs
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map_or(1, std::num::NonZero::get)
                .div_ceil(4)
        })
        .max(1)
}

/// Runs `items` on `jobs` threads, calling `report` (serialised) with each outcome; returns the outcomes in order.
fn run_parallel(
    r: &Runner,
    o: &Options,
    items: &[Scenario],
    jobs: usize,
    report: &(dyn Fn(&Outcome) + Sync),
) -> Result<Vec<Outcome>, String> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Outcome)>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| -> Result<(), String> {
        let mut handles = Vec::new();
        for _ in 0..jobs.min(items.len()).max(1) {
            let h = std::thread::Builder::new()
                .name(String::from("difftest-worker"))
                .spawn_scoped(scope, || {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(s) = items.get(i) else { break };
                        let outcome = r.run_with(s, o);
                        report(&outcome);
                        if let Ok(mut g) = results.lock() {
                            g.push((i, outcome));
                        }
                    }
                })
                .map_err(|e| format!("starting a worker: {e}"))?;
            handles.push(h);
        }
        for h in handles {
            h.join().map_err(|_| String::from("a worker panicked"))?;
        }
        Ok(())
    })?;
    let mut v = results
        .into_inner()
        .map_err(|_| String::from("poisoned results"))?;
    v.sort_by_key(|(i, _)| *i);
    Ok(v.into_iter().map(|(_, o)| o).collect())
}

fn print_outcome(o: &Outcome) {
    if o.passed() {
        let tag = if o.skipped.is_some() { "skip" } else { "ok" };
        let detail = o.skipped.clone().unwrap_or_else(|| o.verdict.label());
        out(&format!("{tag:>4} {} ({detail})\n", o.name));
    } else {
        out(&format!("FAIL {}", o.report()));
    }
}

fn summarise(outcomes: &[Outcome]) -> bool {
    let passed = outcomes
        .iter()
        .filter(|o| o.passed() && o.skipped.is_none())
        .count();
    let skipped = outcomes.iter().filter(|o| o.skipped.is_some()).count();
    let failed: Vec<&Outcome> = outcomes.iter().filter(|o| !o.passed()).collect();
    let listed = outcomes
        .iter()
        .filter(|o| matches!(o.verdict, Verdict::Diverged { id: Some(_), .. }))
        .count();
    out(&format!(
        "\n{} scenarios: {passed} passed ({listed} diverging as registered), {skipped} skipped, {} failed\n",
        outcomes.len(),
        failed.len()
    ));
    for f in &failed {
        out(&format!(
            "  FAILED {}: {}\n",
            f.name,
            f.error.clone().unwrap_or_else(|| f.verdict.label())
        ));
    }
    failed.is_empty()
}

fn cmd_run(a: &Args) -> Result<bool, String> {
    let (r, o) = runner(a)?;
    if a.suite.as_deref() == Some("generated") {
        return run_generated(a, &r, &o);
    }
    let items = select(a)?;
    let outcomes = run_parallel(&r, &o, &items, jobs(a), &print_outcome)?;
    Ok(summarise(&outcomes))
}

/// The predicate the shrinker keeps: an unlisted difference touching the same channels as the original.
fn same_failure(r: &Runner, o: &Options, original: &Outcome) -> impl FnMut(&Scenario) -> bool {
    let channels = |out: &Outcome| -> Vec<String> {
        let mut v: Vec<String> = out
            .variants
            .iter()
            .flat_map(|vr| {
                vr.differences
                    .iter()
                    .map(|d| d.what.rsplit(' ').next().unwrap_or_default().to_owned())
            })
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let want = channels(original);
    let mut quiet = o.clone();
    quiet.artifacts = None;
    move |s: &Scenario| {
        let out = r.run_with(s, &quiet);
        out.error.is_none()
            && matches!(out.verdict, Verdict::Diverged { id: None, .. })
            && channels(&out).iter().any(|c| want.contains(c))
    }
}

fn shrink_and_write(
    a: &Args,
    r: &Runner,
    o: &Options,
    s: &Scenario,
    outcome: &Outcome,
) -> Result<(), String> {
    let mut pred = same_failure(r, o, outcome);
    let shrunk = shrink::shrink(s, &mut pred, a.max_shrink_runs);
    let rerun = r.run_with(&shrunk.scenario, o);
    let dir = o
        .artifacts
        .clone()
        .unwrap_or_else(|| workspace().join("target/difftest/out"));
    let name = format!("{}.shrunk", shrunk.scenario.name.replace('/', "_"));
    let base = dir.join(&name);
    std::fs::create_dir_all(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    std::fs::write(base.join("scenario.yaml"), shrunk.scenario.to_yaml())
        .map_err(|e| e.to_string())?;
    let root = r.scenario_root(&shrunk.scenario)?;
    let script = shrink::repro_script(&shrunk.scenario, r, &root);
    std::fs::write(base.join("repro.sh"), script).map_err(|e| e.to_string())?;
    out(&format!(
        "  shrunk in {} runs to {} hook(s), {} file(s), {} step(s): {}\n{}",
        shrunk.runs,
        shrink::hook_count(&shrunk.scenario),
        shrunk.scenario.work.files.len(),
        shrunk.scenario.steps.len(),
        base.display(),
        rerun.report()
    ));
    Ok(())
}

fn run_generated(a: &Args, r: &Runner, o: &Options) -> Result<bool, String> {
    let probe = o.probe.is_some();
    let indices: Vec<u64> = match a.case {
        Some(i) => vec![i],
        None => (0..a.cases).collect(),
    };
    let items: Vec<Scenario> = indices
        .iter()
        .map(|&i| generate::generate(a.seed, i, probe))
        .collect();
    out(&format!(
        "generated suite: seed {} ({} cases)\n",
        a.seed,
        items.len()
    ));
    let quiet = |oc: &Outcome| {
        if !oc.passed() {
            print_outcome(oc);
        }
    };
    let outcomes = run_parallel(r, o, &items, jobs(a), &quiet)?;
    if !a.no_shrink {
        for (s, oc) in items.iter().zip(&outcomes) {
            if !oc.passed() && oc.error.is_none() {
                out(&format!("shrinking {}\n", s.name));
                shrink_and_write(a, r, o, s, oc)?;
            }
        }
    }
    Ok(summarise(&outcomes))
}

fn cmd_list(a: &Args) -> Result<bool, String> {
    for s in select(a)? {
        let id = s.expect_divergence.clone().unwrap_or_default();
        out(&format!(
            "{}\t{:?}\t{id}\t{}\n",
            s.name,
            s.status,
            s.path.display()
        ));
    }
    Ok(true)
}

fn cmd_register(a: &Args) -> Result<bool, String> {
    let rep = register::check(&workspace())?;
    out(&rep.render());
    // Every scenario of the corpus that names a register ID must name a real one.
    let corpus = load_scenarios(&workspace().join("tests/differential/scenarios"))?;
    let mut ok = rep.is_clean(a.strict);
    for s in &corpus {
        if let Some(id) = &s.expect_divergence
            && !rep.ids.iter().any(|i| i.entry.id == *id)
        {
            out(&format!(
                "PROBLEM: {}: expect_divergence {id} is not in COMPATIBILITY §5\n",
                s.path.display()
            ));
            ok = false;
        }
    }
    Ok(ok)
}

fn cmd_generate(a: &Args) -> Result<bool, String> {
    let dir = a.out.clone().ok_or("generate needs --out DIR")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for i in 0..a.cases {
        let s = generate::generate(a.seed, i, !a.no_probe);
        let p = dir.join(format!("{}-{i}.yaml", a.seed));
        std::fs::write(&p, s.to_yaml()).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    out(&format!(
        "wrote {} scenarios to {}\n",
        a.cases,
        dir.display()
    ));
    Ok(true)
}

fn cmd_shrink(a: &Args) -> Result<bool, String> {
    let (r, o) = runner(a)?;
    let items = select(&clone_sel(a))?;
    let mut ok = true;
    for s in &items {
        let oc = r.run_with(s, &o);
        if oc.passed() {
            out(&format!(
                "{}: nothing to shrink ({})\n",
                s.name,
                oc.verdict.label()
            ));
            continue;
        }
        ok = false;
        shrink_and_write(a, &r, &o, s, &oc)?;
    }
    Ok(ok)
}

/// The selection part of `a` (for `shrink`, whose input is `--file`/`--scenario`).
fn clone_sel(a: &Args) -> Args {
    Args {
        suite: a.files.is_empty().then(|| String::from("all")),
        scenarios: a.scenarios.clone(),
        files: a.files.clone(),
        tags: a.tags.clone(),
        ..Args::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args() {
        let v: Vec<OsString> = [
            "run", "--suite", "seed", "--brand", "snp", "--seed", "abc", "-j", "3",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        let a = parse_args(&v).unwrap();
        assert_eq!(a.suite.as_deref(), Some("seed"));
        assert_eq!(a.brand, vec![Brand::Snp]);
        assert_eq!(a.jobs, Some(3));
        assert_eq!(a.seed, parse_seed("abc"));
        assert_eq!(parse_seed("42"), 42);
        assert!(parse_args(&[OsString::from("run"), OsString::from("--bogus")]).is_err());
        assert!(parse_args(&[]).is_err());
    }
}
