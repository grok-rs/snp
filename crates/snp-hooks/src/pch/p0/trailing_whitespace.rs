//! `trailing-whitespace` (console script `trailing-whitespace-fixer`, `trailing_whitespace_fixer.py`; 09 §4.A).
//!
//! One epoch v3.0.0…v6.0.0. Builtin variant (09 §7.2): a file named `x.` is not markdown by default (upstream's
//! default `md_exts` is `['.']`), plus `--check`.
//!
//! Owner: WP-26.

use std::ffi::OsStr;
use std::sync::Arc;

use snp_argparse::{ActionSpec, DefaultValue, Namespace, ParserSpec};
use snp_hooks_data::Epoch;

use super::common::{self, epoch_in, filenames, print_name, read_all, rewrite};
use super::kernels;
use crate::framework::{
    self, FileStep, PyOut, append, files_positional, store, store_true, with_default, with_help,
    with_metavar,
};
use crate::{HookCx, NativeHook, Outcome};

/// The console script.
const PROG: &str = "trailing-whitespace-fixer";

const NO_MD: ActionSpec = ActionSpec {
    hidden: true,
    ..store_true(
        &["--no-markdown-linebreak-ext"],
        "no_markdown_linebreak_ext",
    )
};
const MD_EXT: ActionSpec = with_metavar(
    with_help(
        with_default(
            append(&["--markdown-linebreak-ext"], "markdown_linebreak_ext"),
            DefaultValue::EmptyList,
        ),
        Some("Markdown extensions (or *) to not strip linebreak spaces.  default: %(default)s"),
    ),
    "*|EXT[,EXT,...]",
);
const CHARS: ActionSpec = with_help(
    store(&["--chars"], "chars"),
    Some(
        "The set of characters to strip from the end of lines.  Defaults to all whitespace \
         characters.",
    ),
);
const CHECK: ActionSpec = with_help(
    store_true(&["--check"], "check"),
    Some("Report the files that would be fixed without changing them."),
);

static UPSTREAM_ACTIONS: [ActionSpec; 4] = [
    NO_MD,
    MD_EXT,
    CHARS,
    files_positional(Some("Filenames to fix")),
];
static BUILTIN_ACTIONS: [ActionSpec; 5] = [
    NO_MD,
    MD_EXT,
    CHARS,
    CHECK,
    files_positional(Some("Filenames to fix")),
];

/// `trailing_whitespace_fixer.main`'s parser.
pub(crate) static SPEC: ParserSpec = ParserSpec {
    prog_suffix: "",
    description: None,
    epilog: None,
    actions: &UPSTREAM_ACTIONS,
    allow_abbrev: true,
    add_help: true,
};
static BUILTIN_SPEC: ParserSpec = ParserSpec {
    actions: &BUILTIN_ACTIONS,
    ..SPEC
};

/// The native (`builtin`: the `repo: builtin` variant).
#[derive(Debug)]
pub(crate) struct TrailingWhitespace {
    builtin: bool,
}

/// The pre-commit-hooks native.
pub(crate) static UPSTREAM: TrailingWhitespace = TrailingWhitespace { builtin: false };
/// The `repo: builtin` variant.
pub(crate) static BUILTIN: TrailingWhitespace = TrailingWhitespace { builtin: true };

/// `x.lower()` of a str argument (Unicode lowercase; undecodable bytes lowercased as ASCII).
fn lower(b: &[u8]) -> Vec<u8> {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_lowercase().into_bytes(),
        Err(_) => b.to_ascii_lowercase(),
    }
}

/// The markdown extension set (`main`, `:74-91`): `Err` = the `parser.error` message.
fn markdown_exts(ns: &Namespace, builtin: bool) -> Result<(bool, Vec<Vec<u8>>), String> {
    let md_args = ns.list("markdown_linebreak_ext");
    if md_args.iter().any(|a| a.is_empty()) {
        return Err("--markdown-linebreak-ext requires a non-empty argument".to_owned());
    }
    let all = md_args.iter().any(|a| a.as_os_str() == "*");
    if builtin && md_args.is_empty() {
        return Ok((all, Vec::new()));
    }
    let joined: Vec<&[u8]> = md_args.iter().map(|a| a.as_encoded_bytes()).collect();
    let joined = joined.join(&b","[..]);
    let mut exts = Vec::new();
    for x in joined.split(|&b| b == b',') {
        let low = lower(x);
        let trimmed = low
            .iter()
            .position(|&b| b != b'.')
            .and_then(|i| low.get(i..))
            .unwrap_or_default();
        let mut ext = vec![b'.'];
        ext.extend_from_slice(trimmed);
        if ext
            .get(1..)
            .is_some_and(|rest| rest.iter().any(|b| matches!(b, b'.' | b'/' | b'\\' | b':')))
        {
            let shown = snp_pycompat::repr::repr_str(&String::from_utf8_lossy(&ext));
            return Err(format!(
                "bad --markdown-linebreak-ext extension {shown} (has . / \\ :)\n  (probably \
                 filename; use '--markdown-linebreak-ext=EXT')"
            ));
        }
        exts.push(ext);
    }
    Ok((all, exts))
}

impl TrailingWhitespace {
    fn spec(&self) -> &'static ParserSpec {
        if self.builtin { &BUILTIN_SPEC } else { &SPEC }
    }
}

impl NativeHook for TrailingWhitespace {
    fn id(&self) -> &'static str {
        "trailing-whitespace"
    }

    fn prog(&self) -> &'static str {
        PROG
    }

    fn preflight(&self, epoch: Epoch, _args: &[Arc<str>]) -> Result<(), String> {
        if self.builtin {
            Ok(())
        } else {
            epoch_in(epoch, 1)
        }
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        let ns = match common::parse(cx, self.spec(), PROG, argv) {
            Ok(ns) => ns,
            Err(o) => return o,
        };
        let mut out = PyOut::default();
        if ns.flag("no_markdown_linebreak_ext") {
            out.print("--no-markdown-linebreak-ext now does nothing!");
        }
        let (all_markdown, md_exts) = match markdown_exts(&ns, self.builtin) {
            Ok(v) => v,
            Err(msg) => {
                out.eprint_bytes(&framework::parser_error(
                    self.spec(),
                    PROG,
                    cx.columns,
                    cx.env,
                    &msg,
                ));
                return Outcome {
                    code: 2,
                    output: out.finish(),
                    ..Outcome::default()
                };
            }
        };
        let chars: Option<Vec<u8>> = ns.value("chars").map(|c| c.as_encoded_bytes().to_vec());
        let check = self.builtin && ns.flag("check");
        let files = filenames(&ns);
        let mut step = |name: &OsStr, out: &mut PyOut| {
            let low = lower(name.as_encoded_bytes());
            let ext = kernels::splitext_ext(&low);
            let md = all_markdown || md_exts.iter().any(|e| e.as_slice() == ext);
            let content = match read_all(cx, name) {
                Ok(c) => c,
                Err(e) => return FileStep::Crash(e),
            };
            let Some(new) = kernels::trailing_whitespace(&content, md, chars.as_deref()) else {
                return FileStep::Unchanged;
            };
            if check {
                print_name(out, b"Would fix ", name, b"");
                return FileStep::Failed;
            }
            if let Err(e) = rewrite(cx, name, &new) {
                return FileStep::Crash(e);
            }
            print_name(out, b"Fixing ", name, b"");
            framework::repo_path(name).map_or(FileStep::Failed, FileStep::Changed)
        };
        let mut outcome = framework::for_each_file(cx, &files, &mut out, &mut step);
        outcome.reads_only = check;
        framework::finish(outcome, out)
    }
}
