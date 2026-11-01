//! `repo: builtin` ⇄ `pre-commit/pre-commit-hooks` conversion (09 §7.1).
//!
//! Both directions edit the config text in place (spans from snp-yaml's composer), so formatting, comments, key
//! order and line endings survive:
//!
//! * `to_builtin`: every repo whose `repo` is the pre-commit-hooks URL (`snp_hooks_data::is_pre_commit_hooks_url`)
//!   and `rev` are one-line scalars and whose hooks are all builtin ids without `entry`, `additional_dependencies` or
//!   `language` becomes `repo: builtin`; its `rev` is removed and a comment line `# was: <url>@<rev>` (plus any comment that
//!   followed the rev) is written above the repo item.
//! * `to_upstream`: every `repo: builtin` becomes the URL and rev of its `# was:` comment, or the canonical URL and
//!   the newest known tag; `language: system` hook keys (valid only for builtin) are removed. Without a `# was:`
//!   comment a `# was: builtin` marker is written instead, so converting back restores the original bytes.
//!
//! An item sharing its line with another (`repos: [{…}, {…}]`) keeps no comment, and a `repo` or `rev` whose key and
//! value are on different lines, or that carries an anchor or tag or is an alias, is left alone. The rewritten text
//! must compose again, else nothing changes (exit 1); configs with YAML line breaks other than `\n`/`\r\n` are
//! refused.
//!
//! The round trip `to_upstream(to_builtin(x)) == x` is byte-exact when `rev` directly follows `repo` (the layout
//! pre-commit's own tools write). Otherwise the parsed config is the same but the restored `rev` line sits right
//! after `repo` (a `rev` written before `repo` or after `hooks` moves).
//!
//! Owner: WP-34.

use snp_core::failure::Failure;
use snp_yaml::{Document, NodeId, NodeKind};

/// The canonical upstream URL.
const UPSTREAM: &str = "https://github.com/pre-commit/pre-commit-hooks";

/// The comment marker.
const WAS: &str = "# was: ";

fn failure(msg: &str) -> Failure {
    Failure::SystemExit {
        code: 1,
        stderr: format!("snp migrate: {msg}\n").into_bytes(),
    }
}

/// The newest pre-commit-hooks tag snp has data for (`GENERATED_FROM` = `… v3.0.0..v6.0.0 @ <sha>`).
fn latest_tag() -> &'static str {
    snp_hooks_data::GENERATED_FROM
        .split_once("..")
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .filter(|t| snp_hooks_data::profile_for_rev(t).is_some())
        .unwrap_or("v6.0.0")
}

/// The hook ids of `repo: builtin` (the `-   id: …` lines of the generated manifest).
fn builtin_ids() -> Vec<&'static str> {
    snp_hooks_data::BUILTIN_MANIFEST_YAML
        .lines()
        .filter_map(|l| l.strip_prefix("-   id: "))
        .map(str::trim)
        .collect()
}

/// One text edit: replace `start..end` with `text`.
#[derive(Debug, Clone)]
struct Edit {
    start: usize,
    end: usize,
    text: String,
}

/// Navigation over a composed document.
struct Doc<'a> {
    text: &'a str,
    composed: Document,
    nl: &'static str,
}

impl<'a> Doc<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, Failure> {
        let text = std::str::from_utf8(bytes)
            .map_err(|e| failure(&format!("the config is not valid UTF-8: {e}")))?;
        // The edits are line based (`\n` / `\r\n`); YAML's other line breaks would let a written comment end early.
        if text.contains(['\u{85}', '\u{2028}', '\u{2029}'])
            || text.replace("\r\n", "\n").contains('\r')
        {
            return Err(failure(
                "the config uses line breaks other than \\n and \\r\\n; convert them first",
            ));
        }
        let lim = snp_yaml::Limits::default();
        let doc = snp_yaml::compose(text, snp_yaml::Schema::PyYaml11Safe, &lim)
            .map_err(|e| failure(&e.to_string()))?;
        let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
        Ok(Self {
            text,
            composed: doc,
            nl,
        })
    }

    fn node(&self, id: NodeId) -> Option<&snp_yaml::Node> {
        self.composed.nodes.get(usize::try_from(id.0).ok()?)
    }

    fn scalar(&self, id: NodeId) -> Option<&str> {
        match &self.node(id)?.kind {
            NodeKind::Scalar(s) => Some(s),
            _ => None,
        }
    }

    fn pairs(&self, id: NodeId) -> &[(NodeId, NodeId)] {
        match self.node(id).map(|n| &n.kind) {
            Some(NodeKind::Map(p)) => p,
            _ => &[],
        }
    }

    fn items(&self, id: NodeId) -> &[NodeId] {
        match self.node(id).map(|n| &n.kind) {
            Some(NodeKind::Seq(i)) => i,
            _ => &[],
        }
    }

    /// The `(key, value)` of `key` in map `id`.
    fn get(&self, id: NodeId, key: &str) -> Option<(NodeId, NodeId)> {
        self.pairs(id)
            .iter()
            .copied()
            .find(|(k, _)| self.scalar(*k) == Some(key))
    }

    fn span(&self, id: NodeId) -> std::ops::Range<usize> {
        self.node(id).map_or(0..0, |n| n.span.clone())
    }

    fn raw(&self, id: NodeId) -> &str {
        self.text.get(self.span(id)).unwrap_or("")
    }

    /// Whether node `id` is written in place inside `within` without an anchor, tag or alias: an alias's span is its
    /// anchor's (elsewhere), and replacing or removing an anchored node would leave its aliases undefined.
    fn plain_in(&self, id: NodeId, within: NodeId) -> bool {
        let (span, outer) = (self.span(id), self.span(within));
        self.node(id).is_some_and(|n| n.anchor.is_none())
            && !self.raw(id).starts_with(['&', '!', '*'])
            && outer.start <= span.start
            && span.end <= outer.end
    }

    /// Whether map `id` is a flow mapping (`{…}`).
    fn is_flow(&self, id: NodeId) -> bool {
        self.raw(id).starts_with('{')
    }

    /// The start of the line holding byte `at`.
    fn line_start(&self, at: usize) -> usize {
        self.text
            .get(..at)
            .and_then(|s| s.rfind('\n'))
            .map_or(0, |i| i.saturating_add(1))
    }

    /// The byte after the line break ending the line holding `at` (the text's end without one).
    fn next_line(&self, at: usize) -> usize {
        self.text
            .get(at..)
            .and_then(|s| s.find('\n'))
            .map_or(self.text.len(), |i| at.saturating_add(i).saturating_add(1))
    }

    /// The rest of the line after `at` (without the line break).
    fn rest_of_line(&self, at: usize) -> &str {
        let end = self.next_line(at);
        self.text
            .get(at..end)
            .unwrap_or("")
            .trim_end_matches(['\n', '\r'])
    }

    /// Whether `item` is the first thing on its line after the sequence indicator (`  - ` / `-   `), so the line above
    /// it can carry its `# was:` comment unambiguously.
    fn owns_line(&self, item: NodeId) -> bool {
        let start = self.span(item).start;
        self.text
            .get(self.line_start(start)..start)
            .is_some_and(|p| p.chars().all(|c| c == ' ' || c == '-'))
    }

    /// The text `edits` produce, provided it still composes (a rewrite never emits a broken config).
    fn finish(&self, edits: Vec<Edit>) -> Result<Vec<u8>, Failure> {
        let out = self.apply(edits);
        let text = std::str::from_utf8(&out).map_err(|e| failure(&e.to_string()))?;
        snp_yaml::compose(
            text,
            snp_yaml::Schema::PyYaml11Safe,
            &snp_yaml::Limits::default(),
        )
        .map_err(|e| {
            failure(&format!(
                "the rewritten config does not parse ({e}); nothing changed"
            ))
        })?;
        Ok(out)
    }

    /// The `repos` items.
    fn repos(&self) -> Vec<NodeId> {
        let Some(root) = self.composed.root else {
            return Vec::new();
        };
        self.get(root, "repos")
            .map(|(_, v)| self.items(v).to_vec())
            .unwrap_or_default()
    }

    /// The `# was: …` comment line directly above the line of `item`: `(line start, line end, text after "# was: ")`.
    fn was_comment(&self, item: NodeId) -> Option<(usize, usize, &str)> {
        let ls = self.line_start(self.span(item).start);
        if ls == 0 {
            return None;
        }
        let prev = self.line_start(ls.saturating_sub(1));
        let line = self.text.get(prev..ls)?.trim_end_matches(['\n', '\r']);
        let body = line.trim_start().strip_prefix(WAS)?;
        Some((prev, ls, body.trim_end()))
    }

    /// The indentation (spaces) of the line holding `at`.
    fn indent_of_line(&self, at: usize) -> String {
        let ls = self.line_start(at);
        self.text
            .get(ls..)
            .unwrap_or("")
            .chars()
            .take_while(|c| *c == ' ')
            .collect()
    }

    /// The column of `at` as spaces.
    fn column(&self, at: usize) -> String {
        let ls = self.line_start(at);
        std::iter::repeat_n(' ', self.text.get(ls..at).map_or(0, |s| s.chars().count())).collect()
    }

    /// Edits removing the pair `(k, v)` of map `map`; `None` when the layout is not understood.
    fn remove_pair(&self, map: NodeId, k: NodeId, v: NodeId) -> Option<Vec<Edit>> {
        let ks = self.span(k).start;
        let ve = self.span(v).end;
        let pairs = self.pairs(map);
        let pos = pairs.iter().position(|(x, _)| *x == k)?;
        if self.is_flow(map) {
            // `a: 1, rev: x, b: 2` → drop `rev: x, `; the last pair drops the preceding `, `.
            if let Some((nk, _)) = pairs.get(pos.saturating_add(1)) {
                return Some(vec![Edit {
                    start: ks,
                    end: self.span(*nk).start,
                    text: String::new(),
                }]);
            }
            let (_, pv) = pairs.get(pos.checked_sub(1)?)?;
            return Some(vec![Edit {
                start: self.span(*pv).end,
                end: ve,
                text: String::new(),
            }]);
        }
        let ls = self.line_start(ks);
        let before = self.text.get(ls..ks)?;
        if before.trim().is_empty() {
            // The key starts its line: drop the whole line(s) through the value's line. When they end the text
            // without a final line break, the break before them goes instead (the text keeps having none).
            let end = self.next_line(ve);
            let start = if end == self.text.len() && !self.text.ends_with('\n') {
                let head = self.text.get(..ls).unwrap_or("");
                head.strip_suffix("\r\n")
                    .or_else(|| head.strip_suffix('\n'))
                    .unwrap_or(head)
                    .len()
            } else {
                ls
            };
            return Some(vec![Edit {
                start,
                end,
                text: String::new(),
            }]);
        }
        // `- rev: x` as the first pair: the next key moves up.
        let (nk, _) = pairs.get(pos.saturating_add(1))?;
        Some(vec![Edit {
            start: ks,
            end: self.span(*nk).start,
            text: String::new(),
        }])
    }

    /// Applies non-overlapping edits.
    fn apply(&self, mut edits: Vec<Edit>) -> Vec<u8> {
        edits.sort_by_key(|e| (e.start, e.end));
        let mut out = String::new();
        let mut at = 0_usize;
        for e in edits {
            if e.start < at {
                continue;
            }
            out.push_str(self.text.get(at..e.start).unwrap_or(""));
            out.push_str(&e.text);
            at = e.end;
        }
        out.push_str(self.text.get(at..).unwrap_or(""));
        out.into_bytes()
    }
}

/// Whether every hook of a pre-commit-hooks repo can run as `repo: builtin`.
fn convertible(d: &Doc<'_>, hooks: Option<NodeId>, ids: &[&str]) -> bool {
    let Some(hooks) = hooks else { return false };
    let items = d.items(hooks);
    !items.is_empty()
        && items.iter().all(|h| {
            let id_ok = d
                .get(*h, "id")
                .and_then(|(_, v)| d.scalar(v))
                .is_some_and(|id| ids.contains(&id));
            // Any `language` (even `system`, which upstream means "run the installed console script") changes
            // what runs, and converting back would drop it.
            id_ok
                && d.get(*h, "language").is_none()
                && d.get(*h, "entry").is_none()
                && d.get(*h, "additional_dependencies").is_none()
        })
}

/// `snp migrate builtin`: pre-commit-hooks entries → `repo: builtin` (09 §7.1).
///
/// Rewrites `repo: https://github.com/pre-commit/pre-commit-hooks` (any URL form pre-commit accepts for it) with its
/// `rev` into `repo: builtin` when every hook of the block is a builtin id without `entry`,
/// `additional_dependencies` or `language`; other repos and the rest of the text are untouched.
pub fn to_builtin(config_bytes: &[u8]) -> Result<Vec<u8>, Failure> {
    let d = Doc::parse(config_bytes)?;
    let ids = builtin_ids();
    let mut edits = Vec::new();
    for item in d.repos() {
        let Some((_, repo_v)) = d.get(item, "repo") else {
            continue;
        };
        let Some(url) = d.scalar(repo_v) else {
            continue;
        };
        // A remote repo without `rev` is invalid for pre-commit anyway; a multi-line URL or rev cannot go into the
        // one-line `# was:` comment.
        let Some(rev) = d.get(item, "rev") else {
            continue;
        };
        // The key and its value on one line (`rev:\n  v1` would come back as `rev: v1`).
        let one_line = |k: NodeId, v: NodeId| {
            d.text
                .get(d.span(k).start..d.span(v).end)
                .is_some_and(|t| !t.contains(['\n', '\r']))
        };
        let repo_k = d.get(item, "repo").map_or(repo_v, |(k, _)| k);
        if !snp_hooks_data::is_pre_commit_hooks_url(url)
            || ![repo_k, repo_v, rev.0, rev.1]
                .iter()
                .all(|n| d.plain_in(*n, item))
            || !convertible(&d, d.get(item, "hooks").map(|(_, v)| v), &ids)
            || !d.owns_line(item)
            || !one_line(repo_k, repo_v)
            || !one_line(rev.0, rev.1)
        {
            continue;
        }
        let (rk, rv) = rev;
        let Some(rm) = d.remove_pair(item, rk, rv) else {
            continue;
        };
        let mut item_edits = vec![Edit {
            start: d.span(repo_v).start,
            end: d.span(repo_v).end,
            text: "builtin".to_owned(),
        }];
        item_edits.extend(rm);
        let mut was = format!("{}@{}", d.raw(repo_v), d.raw(rv));
        if !d.is_flow(item) {
            let rest = d.rest_of_line(d.span(rv).end);
            if rest.trim_start().starts_with('#') {
                was.push_str(rest);
            }
        }
        let ls = d.line_start(d.span(item).start);
        match d.was_comment(item) {
            Some((start, end, "builtin")) => item_edits.push(Edit {
                start,
                end,
                text: String::new(),
            }),
            _ => item_edits.push(Edit {
                start: ls,
                end: ls,
                text: format!("{}{WAS}{was}{}", d.indent_of_line(ls), d.nl),
            }),
        }
        edits.extend(item_edits);
    }
    d.finish(edits)
}

/// Why [`to_builtin`] leaves each pre-commit-hooks block of `config_bytes` alone because of its hooks (ADR-084): one
/// line per block, `pre-commit-hooks (line N) not converted: <ids> have no builtin` and/or `… hook <id> sets
/// <key>`. Empty when every such block converts (or there is none).
pub fn not_converted(config_bytes: &[u8]) -> Result<Vec<String>, Failure> {
    let d = Doc::parse(config_bytes)?;
    let ids = builtin_ids();
    let mut out = Vec::new();
    for item in d.repos() {
        let Some(url) = d.get(item, "repo").and_then(|(_, v)| d.scalar(v)) else {
            continue;
        };
        if !snp_hooks_data::is_pre_commit_hooks_url(url) {
            continue;
        }
        let Some((_, hooks)) = d.get(item, "hooks") else {
            continue;
        };
        let mut missing: Vec<&str> = Vec::new();
        let mut keyed: Vec<String> = Vec::new();
        for &h in d.items(hooks).iter() {
            let id = d.get(h, "id").and_then(|(_, v)| d.scalar(v)).unwrap_or("");
            if !ids.contains(&id) && !missing.contains(&id) {
                missing.push(id);
            }
            for key in ["language", "entry", "additional_dependencies"] {
                if d.get(h, key).is_some() {
                    keyed.push(format!("hook {id} sets `{key}`"));
                }
            }
        }
        if missing.is_empty() && keyed.is_empty() {
            continue;
        }
        let line = d
            .text
            .get(..d.span(item).start)
            .map_or(0, |t| t.matches('\n').count())
            .saturating_add(1);
        let mut why = Vec::new();
        if !missing.is_empty() {
            let verb = if missing.len() == 1 { "has" } else { "have" };
            why.push(format!("{} {verb} no builtin", missing.join(", ")));
        }
        why.extend(keyed);
        out.push(format!(
            "pre-commit-hooks (line {line}) not converted: {}",
            why.join("; ")
        ));
    }
    Ok(out)
}

/// `snp migrate upstream`: `repo: builtin` → pre-commit-hooks entries.
///
/// Restores the URL and rev recorded by [`to_builtin`]'s `# was:` comment, else uses
/// `https://github.com/pre-commit/pre-commit-hooks` at the newest tag snp knows (`v6.0.0`); `language: system` hook
/// keys are removed (pre-commit would run the hooks as system commands).
pub fn to_upstream(config_bytes: &[u8]) -> Result<Vec<u8>, Failure> {
    let d = Doc::parse(config_bytes)?;
    let mut edits = Vec::new();
    for item in d.repos() {
        let Some((repo_k, repo_v)) = d.get(item, "repo") else {
            continue;
        };
        if d.scalar(repo_v) != Some("builtin")
            || !d.plain_in(repo_k, item)
            || !d.plain_in(repo_v, item)
        {
            continue;
        }
        let was = if d.owns_line(item) {
            d.was_comment(item)
        } else {
            None
        };
        let (url, rev, rest, comment) = match was {
            Some((start, end, body)) if body != "builtin" => {
                // `<url>@<rev>[  # comment]`: the rev is the last `@`-separated word.
                let (main, rest) = match body.find(" #").or_else(|| body.find("\t#")) {
                    Some(i) => (body.get(..i).unwrap_or(body), body.get(i..).unwrap_or("")),
                    None => (body, ""),
                };
                let main = main.trim_end();
                let pad = &body
                    .get(main.len()..body.len().saturating_sub(rest.len()))
                    .unwrap_or("");
                match main.rsplit_once('@') {
                    Some((u, r)) if !u.is_empty() && !r.is_empty() => (
                        u.to_owned(),
                        r.to_owned(),
                        format!("{pad}{rest}"),
                        Some((start, end)),
                    ),
                    _ => (
                        UPSTREAM.to_owned(),
                        latest_tag().to_owned(),
                        String::new(),
                        None,
                    ),
                }
            }
            _ => (
                UPSTREAM.to_owned(),
                latest_tag().to_owned(),
                String::new(),
                None,
            ),
        };
        let (start, end, text) = comment.map_or_else(
            || {
                if !d.owns_line(item) {
                    // Several items share the line: no marker (converting back then keeps the upstream form).
                    let ls = d.line_start(d.span(item).start);
                    return (ls, ls, String::new());
                }
                let ls = d.line_start(d.span(item).start);
                (
                    ls,
                    ls,
                    format!("{}{WAS}builtin{}", d.indent_of_line(ls), d.nl),
                )
            },
            |(start, end)| (start, end, String::new()),
        );
        edits.push(Edit { start, end, text });
        edits.push(Edit {
            start: d.span(repo_v).start,
            end: d.span(repo_v).end,
            text: url,
        });
        if d.get(item, "rev").is_none() {
            if d.is_flow(item) {
                edits.push(Edit {
                    start: d.span(repo_v).end,
                    end: d.span(repo_v).end,
                    text: format!(", rev: {rev}"),
                });
            } else {
                let at = d.next_line(d.span(repo_v).end);
                // At the end of a text without a final line break, the rev line becomes the last line (and has none).
                let (lead, trail) = if at == d.text.len() && !d.text.ends_with('\n') {
                    (d.nl, "")
                } else {
                    ("", d.nl)
                };
                edits.push(Edit {
                    start: at,
                    end: at,
                    text: format!(
                        "{lead}{}rev: {rev}{rest}{trail}",
                        d.column(d.span(repo_k).start)
                    ),
                });
            }
        }
        if let Some((_, hooks)) = d.get(item, "hooks") {
            for h in d.items(hooks) {
                if let Some((lk, lv)) = d.get(*h, "language")
                    && d.scalar(lv) == Some("system")
                    && d.plain_in(lk, *h)
                    && d.plain_in(lv, *h)
                    && let Some(rm) = d.remove_pair(*h, lk, lv)
                {
                    edits.extend(rm);
                }
            }
        }
    }
    d.finish(edits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> String {
        String::from_utf8(to_builtin(s.as_bytes()).unwrap()).unwrap()
    }

    fn u(s: &str) -> String {
        String::from_utf8(to_upstream(s.as_bytes()).unwrap()).unwrap()
    }

    const UP: &str = "repos:\n-   repo: https://github.com/pre-commit/pre-commit-hooks\n    rev: v4.5.0  # frozen: x\n    hooks:\n    -   id: trailing-whitespace\n        args: [--markdown-linebreak-ext=md]\n    -   id: check-yaml\n-   repo: local\n    hooks: []\n";

    #[test]
    fn builtin_block() {
        let out = b(UP);
        assert_eq!(
            out,
            "repos:\n# was: https://github.com/pre-commit/pre-commit-hooks@v4.5.0  # frozen: x\n-   repo: builtin\n    hooks:\n    -   id: trailing-whitespace\n        args: [--markdown-linebreak-ext=md]\n    -   id: check-yaml\n-   repo: local\n    hooks: []\n"
        );
        assert_eq!(u(&out), UP);
    }

    #[test]
    fn upstream_without_comment() {
        let src = "repos:\n  - repo: builtin\n    hooks:\n      - id: end-of-file-fixer\n        language: system\n";
        let out = u(src);
        assert_eq!(
            out,
            format!(
                "repos:\n  # was: builtin\n  - repo: {UPSTREAM}\n    rev: {}\n    hooks:\n      - id: end-of-file-fixer\n",
                latest_tag()
            )
        );
        assert_eq!(latest_tag(), "v6.0.0");
        // Back to builtin: the marker is consumed (`language: system` is not restored: it is builtin's default).
        assert_eq!(
            b(&out),
            "repos:\n  - repo: builtin\n    hooks:\n      - id: end-of-file-fixer\n"
        );
    }

    #[test]
    fn unsupported_hooks_stay() {
        for src in [
            "repos:\n- repo: https://github.com/pre-commit/pre-commit-hooks\n  rev: v4.5.0\n  hooks:\n  - id: check-ast\n",
            "repos:\n- repo: https://github.com/pre-commit/pre-commit-hooks\n  rev: v4.5.0\n  hooks:\n  - id: check-yaml\n    additional_dependencies: [x]\n",
            "repos:\n- repo: https://github.com/pre-commit/pre-commit-hooks\n  rev: v4.5.0\n  hooks:\n  - id: check-yaml\n    language: python\n",
            "repos:\n- repo: https://github.com/someone/pre-commit-hooks\n  rev: v4.5.0\n  hooks:\n  - id: check-yaml\n",
        ] {
            assert_eq!(b(src), src);
        }
    }

    #[test]
    fn flow_items_and_crlf() {
        let src = "repos:\n- {repo: 'https://github.com/pre-commit/pre-commit-hooks', rev: v6.0.0, hooks: [{id: check-json}]}\n";
        let out = b(src);
        assert_eq!(
            out,
            "repos:\n# was: 'https://github.com/pre-commit/pre-commit-hooks'@v6.0.0\n- {repo: builtin, hooks: [{id: check-json}]}\n"
        );
        assert_eq!(u(&out), src);
        let crlf = UP.replace('\n', "\r\n");
        let out = b(&crlf);
        assert!(!out.replace("\r\n", "").contains('\n'));
        assert_eq!(u(&out), crlf);
    }

    #[test]
    fn rev_first_key() {
        let src = "repos:\n- rev: v4.5.0\n  repo: git@github.com:pre-commit/pre-commit-hooks.git\n  hooks:\n  - id: check-json\n";
        let out = b(src);
        assert_eq!(
            out,
            "repos:\n# was: git@github.com:pre-commit/pre-commit-hooks.git@v4.5.0\n- repo: builtin\n  hooks:\n  - id: check-json\n"
        );
    }

    #[test]
    fn errors_and_empty() {
        assert!(to_builtin(b"\xff").is_err());
        assert!(to_builtin(b"a: [").is_err());
        assert_eq!(to_builtin(b"").unwrap(), b"");
        assert_eq!(to_upstream(b"repos: []\n").unwrap(), b"repos: []\n");
    }

    /// A `rev` whose value is on the next line is left alone (it could not come back as written).
    #[test]
    fn rev_value_on_its_own_line_stays() {
        let src = "repos:\n-   repo: https://github.com/pre-commit/pre-commit-hooks\n    rev:\n      v4.5.0\n    hooks:\n    -   id: check-yaml\n";
        assert_eq!(b(src), src);
        let src = "repos:\n-   repo:\n      https://github.com/pre-commit/pre-commit-hooks\n    rev: v4.5.0\n    hooks:\n    -   id: check-yaml\n";
        assert_eq!(b(src), src);
    }

    /// A config without a final line break keeps having none, both ways.
    #[test]
    fn no_final_line_break() {
        let src = "repos:\n-   hooks:\n    -   id: check-yaml\n    repo: https://github.com/pre-commit/pre-commit-hooks\n    rev: v4.5.0";
        let out = b(src);
        assert_eq!(
            out,
            "repos:\n# was: https://github.com/pre-commit/pre-commit-hooks@v4.5.0\n-   hooks:\n    -   id: check-yaml\n    repo: builtin"
        );
        assert_eq!(u(&out), src);
        let src = UP.trim_end_matches('\n');
        assert_eq!(u(&b(src)), src);
    }

    /// `rev` after `hooks`: the parsed config survives the round trip, the `rev` line comes back after `repo`.
    #[test]
    fn rev_after_hooks_moves() {
        let src = "repos:\n-   repo: https://github.com/pre-commit/pre-commit-hooks\n    hooks:\n    -   id: check-yaml\n    rev: v4.5.0\n";
        let back = u(&b(src));
        assert_eq!(
            back,
            "repos:\n-   repo: https://github.com/pre-commit/pre-commit-hooks\n    rev: v4.5.0\n    hooks:\n    -   id: check-yaml\n"
        );
    }
}
