//! lefthook → snp conversion (05 §19).
//!
//! The pipeline of 05 §19.2: locate and parse the lefthook configuration (`load`), decode it with lefthook's weak
//! typing (`model`), flatten every hook's jobs into priority buckets and translate each job (`convert`), then render
//! the `.pre-commit-config.yaml` (`emit`), the optional local overlay (a diff of the conversions without and with
//! `lefthook-local.*`) and the report (§19.10). Nothing is written and git is never run: the binary writes the
//! outputs and sequences `--install`.
//!
//! Owner: WP-34.

mod convert;
mod emit;
mod glob;
mod load;
mod model;
mod shell;
mod value;

use std::fmt::Write as _;
use std::path::PathBuf;

use snp_core::failure::Failure;

use convert::{Conversion, Level, Note};
use emit::OutHook;
use value::LValue;

/// `snp migrate lefthook` options.
#[derive(Debug, Clone)]
pub struct LefthookOpts {
    /// The repository root.
    pub root: PathBuf,
    /// The lefthook config (default: discovered).
    pub config: Option<PathBuf>,
    /// The output path (default: `.snp.yaml`; `.pre-commit-config.yaml` with `--portable`; ADR-087).
    pub output: Option<PathBuf>,
    /// Emit only pre-commit-compatible keys.
    pub portable: bool,
    /// Convert `lefthook-local.yml` into the local overlay.
    pub local: bool,
    /// Fail on any lossy conversion.
    pub strict: bool,
}

/// The conversion result.
#[derive(Debug)]
pub struct Converted {
    /// The new `.pre-commit-config.yaml`.
    pub config_yaml: String,
    /// The local overlay, from `lefthook-local.yml`.
    pub overlay_yaml: Option<String>,
    /// The conversion report.
    pub report: Vec<String>,
    /// Whether anything could not be converted exactly.
    pub lossy: bool,
}

/// Global options lefthook has and the output drops (05 §19.3, last row), with an optional hint.
const DROPPED: [(&str, &str); 12] = [
    ("min_version", ""),
    ("colors", ""),
    ("no_tty", ""),
    (
        "output",
        "; `snp run --summary` / SNP_OUTPUT select snp's output",
    ),
    ("assert_lefthook_installed", ""),
    ("no_auto_install", ""),
    ("skip_lfs", ""),
    ("install_non_git_hooks", ""),
    ("lefthook", ""),
    (
        "rc",
        "; source it in your shell profile (or set SNP_RC) instead",
    ),
    (
        "ai",
        "; agent hooks can call `snp run --hook-stage manual --group <group>`",
    ),
    ("$schema", ""),
];

/// Global options consumed by the conversion itself.
const CONSUMED: [&str; 6] = [
    "extends",
    "source_dir",
    "source_dir_local",
    "glob_matcher",
    "templates",
    "remotes",
];

/// A failure with `snp migrate lefthook: <msg>` on stderr and exit `code`.
fn failure(code: i32, msg: &str) -> Failure {
    // Config values quoted in a message never reach the terminal as control sequences.
    let mut stderr = msg
        .split('\n')
        .map(emit::one_line)
        .collect::<Vec<_>>()
        .join("\n");
    if !stderr.ends_with('\n') {
        stderr.push('\n');
    }
    Failure::SystemExit {
        code,
        stderr: stderr.into_bytes(),
    }
}

/// Notes about the configuration as a whole: dropped options, remotes, local file, LFS.
fn global_notes(o: &LefthookOpts, loaded: &load::Loaded, scope: &str) -> (Vec<Note>, Vec<String>) {
    let mut notes = Vec::new();
    let mut todo = Vec::new();
    let mut note = |level, msg: String| {
        notes.push(Note {
            level,
            scope: scope.to_owned(),
            msg,
        });
    };
    for (k, v) in &loaded.global {
        if CONSUMED.contains(&k.as_str()) {
            continue;
        }
        if let Some((_, hint)) = DROPPED.iter().find(|(d, _)| d == k) {
            if k == "skip_lfs" && matches!(v, LValue::Bool(false)) {
                continue;
            }
            note(Level::Warning, format!("option '{k}' dropped{hint}"));
        } else {
            note(
                Level::Warning,
                format!("key '{k}' ignored (not a lefthook hook or option)"),
            );
        }
    }
    for r in &loaded.remotes {
        let url = r
            .as_map()
            .and_then(|m| m.get("git_url"))
            .and_then(LValue::as_str)
            .unwrap_or("?")
            .to_owned();
        let rf = r
            .as_map()
            .and_then(|m| m.get("ref"))
            .and_then(LValue::as_str)
            .map_or_default(|s| format!("@{s}"));
        note(
            Level::Lossy,
            format!("remote config {url}{rf} not converted; convert its configs and add the hooks"),
        );
        todo.push(format!("snp-migrate: TODO remote {url}{rf}"));
    }
    if !o.local
        && let Some(lp) = load::locate_local(&o.root)
    {
        note(
            Level::Warning,
            format!(
                "{} not converted; use --local to turn it into the local overlay",
                load::display(&o.root, &lp)
            ),
        );
    }
    let lfs = std::fs::read_to_string(o.root.join(".gitattributes"))
        .is_ok_and(|s| s.contains("filter=lfs"))
        || o.root.join(".lfsconfig").exists()
        || o.root.join(".lfs-required").exists();
    let skip_lfs = loaded
        .global
        .get("skip_lfs")
        .is_some_and(|v| value::weak_bool(v, "skip_lfs").unwrap_or(false));
    if lfs && !skip_lfs {
        note(
            Level::Warning,
            "git-lfs detected: lefthook ran the git-lfs hooks itself; after `snp install` run `git lfs update --force` \
             and `snp install` again (or use --install) so they are chained as legacy hooks"
                .to_owned(),
        );
    }
    (notes, todo)
}

/// The `.pre-commit-config.yaml` text.
fn render_config(
    conv: &Conversion,
    sources: &str,
    portable: bool,
    global: &[Note],
    todo: &[String],
) -> String {
    let mut out = String::new();
    writeln!(
        out,
        "# Generated by `snp migrate lefthook` from {}",
        emit::one_line(sources)
    )
    .unwrap_or_default();
    for n in global {
        writeln!(out, "# {}", emit::one_line(&n.line())).unwrap_or_default();
    }
    for t in todo {
        writeln!(out, "# {}", emit::one_line(t)).unwrap_or_default();
    }
    if portable
        && conv
            .stages
            .iter()
            .any(|s| matches!(s.as_str(), "pre-commit" | "pre-push" | "pre-merge-commit"))
    {
        out.push_str("minimum_pre_commit_version: '3.2.0'\n");
    }
    if conv.root_never {
        out.push_str(
            "fail_on_changes: never   # snp-migrate: lefthook default; pre-commit semantics would fail modified files\n",
        );
    }
    if !conv.install_types.is_empty() {
        writeln!(
            out,
            "default_install_hook_types: {}",
            emit::flow_list(&conv.install_types)
        )
        .unwrap_or_default();
    }
    write_repos(&mut out, &conv.hooks);
    out
}

/// `repos:` with one `repo: local` holding `hooks` (`repos: []` when there are none).
fn write_repos(out: &mut String, hooks: &[OutHook]) {
    if hooks.is_empty() {
        out.push_str("repos: []\n");
        return;
    }
    out.push_str("repos:\n  - repo: local\n    hooks:\n");
    for h in hooks {
        emit::write_hook(out, h, 6);
    }
}

/// The value an overlay sets to restore a key the local config removed (`None` = not expressible).
fn default_of(key: &str) -> Option<&'static str> {
    Some(match key {
        "files" => "''",
        "exclude" => "'^$'",
        "types" => "[file]",
        "types_or" | "exclude_types" | "groups" => "[]",
        "pass_filenames" => "true",
        "always_run" | "require_serial" | "fail_fast" | "verbose" | "stage_fixed" | "use_stdin"
        | "interactive" | "skip" | "only" => "false",
        _ => return None,
    })
}

/// The overlay of 05 §19.9: the differences between the conversions without and with the local config.
fn render_overlay(
    main: &Conversion,
    full: &Conversion,
    local_name: &str,
    notes: &mut Vec<Note>,
) -> String {
    let mut patches: Vec<(String, Vec<(String, String)>)> = Vec::new();
    let mut extra: Vec<OutHook> = Vec::new();
    let mut matched = vec![false; main.hooks.len()];
    for (f, fstage) in full.hooks.iter().zip(&full.stages) {
        let Some(mi) = main
            .hooks
            .iter()
            .zip(&main.stages)
            .position(|(m, s)| m.id == f.id && s == fstage)
        else {
            let mut h = f.clone();
            h.comments.clear();
            extra.push(h);
            continue;
        };
        if let Some(slot) = matched.get_mut(mi) {
            *slot = true;
        }
        let Some(m) = main.hooks.get(mi) else {
            continue;
        };
        let mf = emit::hook_fields(m);
        let ff = emit::hook_fields(f);
        let mut patch: Vec<(String, String)> = Vec::new();
        for (k, v) in &ff {
            let old = mf.iter().find(|(x, _)| x == k).map(|(_, w)| w);
            if old == Some(v) || *k == "id" {
                continue;
            }
            if *k == "entry"
                && !m.entry.starts_with("sh -c ")
                && !f.entry.starts_with("sh -c ")
                && f.entry.contains(&m.entry)
            {
                let tpl = f.entry.replacen(&m.entry, "{entry}", 1);
                patch.push(((*k).to_owned(), emit::scalar(&tpl, false)));
                continue;
            }
            if *k == "env" {
                let added: Vec<(String, String)> = f
                    .env
                    .iter()
                    .filter(|e| !m.env.contains(e))
                    .cloned()
                    .collect();
                if !added.is_empty() {
                    patch.push(("env".to_owned(), emit::flow_map(&added)));
                }
                continue;
            }
            patch.push(((*k).to_owned(), v.clone()));
        }
        for (k, _) in &mf {
            if ff.iter().any(|(x, _)| x == k) {
                continue;
            }
            match default_of(k) {
                Some(d) => patch.push(((*k).to_owned(), d.to_owned())),
                None => notes.push(Note {
                    level: Level::Lossy,
                    scope: local_name.to_owned(),
                    msg: format!("the overlay cannot remove key '{k}' of hook '{}'", m.id),
                }),
            }
        }
        if f.env.len() < m.env.len() || m.env.iter().any(|e| !f.env.iter().any(|x| x.0 == e.0)) {
            notes.push(Note {
                level: Level::Lossy,
                scope: local_name.to_owned(),
                msg: format!("the overlay cannot remove env variables of hook '{}'", m.id),
            });
        }
        if !patch.is_empty() {
            merge_patch(&mut patches, &m.id, patch, local_name, notes);
        }
    }
    for (m, seen) in main.hooks.iter().zip(&matched) {
        if !*seen {
            merge_patch(
                &mut patches,
                &m.id,
                vec![("skip".to_owned(), "true".to_owned())],
                local_name,
                notes,
            );
        }
    }
    let mut out = String::new();
    writeln!(
        out,
        "# Generated by `snp migrate lefthook --local` from {}",
        emit::one_line(local_name)
    )
    .unwrap_or_default();
    out.push_str(
        "# Personal overrides for snp (never read by pre-commit); add this file to .gitignore.\n",
    );
    if patches.is_empty() {
        out.push_str("hooks: {}\n");
    } else {
        out.push_str("hooks:\n");
        for (id, patch) in &patches {
            writeln!(out, "  {}:", emit::scalar(id, false)).unwrap_or_default();
            for (k, v) in patch {
                writeln!(out, "    {k}: {v}").unwrap_or_default();
            }
        }
    }
    if full.root_never != main.root_never {
        writeln!(
            out,
            "fail_on_changes: {}",
            if full.root_never { "never" } else { "always" }
        )
        .unwrap_or_default();
    }
    for t in &full.install_types {
        if !main.install_types.contains(t) {
            notes.push(Note {
                level: Level::Warning,
                scope: local_name.to_owned(),
                msg: format!(
                    "the local config adds hooks for '{t}': install it with `snp install -t {t}`"
                ),
            });
        }
    }
    if !extra.is_empty() {
        write_repos(&mut out, &extra);
    }
    out
}

/// Adds a patch for `id`; the same id in several stages must get the same patch.
fn merge_patch(
    patches: &mut Vec<(String, Vec<(String, String)>)>,
    id: &str,
    patch: Vec<(String, String)>,
    local_name: &str,
    notes: &mut Vec<Note>,
) {
    match patches.iter().find(|(i, _)| i == id) {
        Some((_, p)) if *p != patch => notes.push(Note {
            level: Level::Lossy,
            scope: local_name.to_owned(),
            msg: format!("hook id '{id}' is changed differently in several stages; only the first change is kept"),
        }),
        Some(_) => {}
        None => patches.push((id.to_owned(), patch)),
    }
}

/// 05 §19 (pure: no git, no writes). `snp migrate lefthook --install` is sequenced by the binary: write the outputs
/// atomically → remove lefthook shims (restore `<hook>.old`), delete `.git/info/lefthook*` → `git lfs update
/// --force` when LFS is detected → `snp_cmd::install` for every emitted stage (which chains the git-lfs hooks as
/// `.legacy`).
///
/// Reproduces lefthook's `internal/config/loader.go` lookup and merge (lefthook 2.x: `lefthook.yml` …
/// `.config/lefthook.toml`, `extends`, `lefthook-local.*`) and converts its hooks per 05 §19.3–§19.9. Errors (exit 1,
/// nothing to write): no or unparsable configuration, a value of the wrong type. With `strict`, a lossy conversion
/// fails with exit 2 and the report on stderr.
pub fn lefthook(o: &LefthookOpts) -> Result<Converted, Failure> {
    let config = o.config.as_deref();
    let main_loaded = match load::load(&o.root, config, false) {
        Ok(l) => Some(l),
        // Local-only mode (no main config): everything goes into the overlay.
        Err(_)
            if o.local
                && config.is_none()
                && load::locate_local(&o.root).is_some()
                && !load::has_main(&o.root) =>
        {
            None
        }
        Err(e) => return Err(failure(1, &format!("snp migrate lefthook: {}", e.0))),
    };
    let copts = convert::Opts {
        root: &o.root,
        portable: o.portable,
    };
    let empty = load::Loaded::empty();
    let base = main_loaded.as_ref().unwrap_or(&empty);
    let main_conv = convert::convert(base, copts)
        .map_err(|e| failure(1, &format!("snp migrate lefthook: {e}")))?;
    let scope = if base.main.is_empty() {
        "lefthook"
    } else {
        base.main.as_str()
    };
    let (mut global, todo) = global_notes(o, base, scope);
    let sources = if base.files.is_empty() {
        "lefthook".to_owned()
    } else {
        base.files.join(", ")
    };
    let config_yaml = render_config(&main_conv, &sources, o.portable, &global, &todo);
    let mut notes: Vec<Note> = main_conv.notes.clone();
    let mut overlay_yaml = None;
    let mut summary_conv = main_conv.clone();
    if o.local {
        match load::locate_local(&o.root) {
            None => global.push(Note {
                level: Level::Warning,
                scope: scope.to_owned(),
                msg: "--local: no lefthook-local config found; no overlay written".to_owned(),
            }),
            Some(lp) => {
                let full_loaded = load::load(&o.root, config, true)
                    .map_err(|e| failure(1, &format!("snp migrate lefthook: {}", e.0)))?;
                let full = convert::convert(&full_loaded, copts)
                    .map_err(|e| failure(1, &format!("snp migrate lefthook: {e}")))?;
                for n in &full.notes {
                    if !notes.contains(n) {
                        notes.push(n.clone());
                    }
                }
                let local_name = load::display(&o.root, &lp);
                let mut onotes = Vec::new();
                overlay_yaml = Some(render_overlay(&main_conv, &full, &local_name, &mut onotes));
                notes.extend(onotes);
                summary_conv = full;
            }
        }
    }
    let mut all: Vec<Note> = global;
    all.extend(notes);
    let warnings = all.iter().filter(|n| n.level == Level::Warning).count();
    let lossy = all.iter().filter(|n| n.level == Level::Lossy).count();
    let mut report: Vec<String> = all.iter().map(Note::line).collect();
    report.push(format!(
        "Converted {} jobs from {} lefthook hooks into {} hooks ({warnings} warnings, {lossy} lossy conversions).",
        summary_conv.jobs,
        summary_conv.lefthook_hooks,
        summary_conv.hooks.len()
    ));
    if o.strict && lossy > 0 {
        let mut text = report.join("\n");
        text.push_str(
            "\nsnp migrate lefthook: --strict: the conversion is lossy; nothing written\n",
        );
        return Err(failure(2, &text));
    }
    Ok(Converted {
        config_yaml,
        overlay_yaml,
        report,
        lossy: lossy > 0,
    })
}
