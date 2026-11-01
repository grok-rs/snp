//! The legacy stage names a manifest uses, for `clientlib.warn_for_stages_on_repo_init` (`clientlib.py:84-109`).
//!
//! pre-commit re-loads the new clone's `.pre-commit-hooks.yaml` with `MINIMAL_MANIFEST_SCHEMA` (a list of maps with a
//! string `id` and an optional `stages` list of strings), collects the names that `transform_stage` would rename
//! (`commit`, `push`, `merge-commit`) in first-seen order, and logs one warning; any load error is swallowed ("they'll
//! get a better error message when it actually loads!"). snp-repo has no YAML parser of its own (the §1.2 DAG gives it
//! snp-config, which only exposes the *transformed* stages), so this module scans the manifest text for the
//! `stages` keys of the hook mappings: block (`- commit`) and flow (`[commit, push]`) sequences, plain and quoted
//! scalars. Wherever the minimal schema would reject the document (a `stages` value that is not a list of strings)
//! the scan reports nothing, exactly like pre-commit; constructs it cannot follow (anchors, aliases, tags, flow
//! mappings, block scalars) also report nothing — such manifests do not occur in the wild.
//!
//! Owner: WP-31.

/// The names `clientlib._STAGES` renames, as `&'static str` for the warning text.
const LEGACY: [&str; 3] = ["commit", "push", "merge-commit"];

/// The legacy stage names of `manifest` in first-seen order, deduplicated; empty when there are none or when the
/// minimal manifest load would fail.
pub(crate) fn legacy_stages(manifest: &[u8]) -> Vec<&'static str> {
    let text = manifest.strip_prefix(b"\xef\xbb\xbf").unwrap_or(manifest);
    let Ok(text) = std::str::from_utf8(text) else {
        return Vec::new();
    };
    if !text.contains("commit") && !text.contains("push") {
        return Vec::new();
    }
    scan(text).unwrap_or_default()
}

/// `None` = the minimal load fails (or the text is beyond this scanner): no warning.
fn scan(text: &str) -> Option<Vec<&'static str>> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found: Vec<&'static str> = Vec::new();
    let mut i = 0usize;
    while let Some(raw) = lines.get(i) {
        i = i.saturating_add(1);
        let line = strip_comment(raw);
        let Some((col, rest)) = stages_key(line) else {
            continue;
        };
        let rest = rest.trim();
        let items = if let Some(flow) = rest.strip_prefix('[') {
            let (items, next) = flow_items(flow, &lines, i)?;
            i = next;
            items
        } else if rest.is_empty() {
            let (items, next) = block_items(col, &lines, i)?;
            i = next;
            items
        } else {
            // `stages: commit` / `stages: null`: `check_array` fails, the whole minimal load fails.
            return None;
        };
        for item in items {
            let s = scalar(item)?;
            if let Some(legacy) = LEGACY.iter().find(|l| **l == s)
                && !found.contains(legacy)
            {
                found.push(legacy);
            }
        }
    }
    Some(found)
}

/// The line without a trailing `# comment` (a `#` at the start or after whitespace, outside quotes).
fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut prev_space = true;
    for (at, c) in line.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            None if c == '\'' || c == '"' => quote = Some(c),
            None if c == '#' && prev_space => return line.get(..at).unwrap_or(line),
            Some(_) | None => {}
        }
        prev_space = c == ' ' || c == '\t';
    }
    line
}

/// `(column, value text)` when `line` holds a `stages:` mapping key (possibly after `- ` sequence indicators).
fn stages_key(line: &str) -> Option<(usize, &str)> {
    let mut rest = line;
    let mut col = 0usize;
    loop {
        let trimmed = rest.trim_start_matches(' ');
        col = col.saturating_add(rest.len().saturating_sub(trimmed.len()));
        rest = trimmed;
        if let Some(after) = rest.strip_prefix('-')
            && (after.is_empty() || after.starts_with(' '))
        {
            rest = after;
            col = col.saturating_add(1);
            continue;
        }
        break;
    }
    let after_key = ["stages", "'stages'", "\"stages\""]
        .iter()
        .find_map(|k| rest.strip_prefix(k))?;
    let value = after_key.trim_start_matches(' ').strip_prefix(':')?;
    (value.is_empty() || value.starts_with(' ')).then_some((col, value))
}

/// The items of a flow sequence whose `[` was already consumed, possibly spanning lines; `(items, next line)`.
fn flow_items<'t>(
    first: &'t str,
    lines: &[&'t str],
    mut next: usize,
) -> Option<(Vec<&'t str>, usize)> {
    let mut items = Vec::new();
    let mut chunk = first;
    loop {
        let (done, consumed) = split_flow(chunk, &mut items)?;
        if done {
            let tail = chunk.get(consumed..).unwrap_or_default().trim();
            return tail.is_empty().then_some((items, next));
        }
        chunk = strip_comment(lines.get(next)?);
        next = next.saturating_add(1);
    }
}

/// Pushes the comma-separated items of `chunk` up to a closing `]`; `(closed, bytes consumed)`.
fn split_flow<'t>(chunk: &'t str, items: &mut Vec<&'t str>) -> Option<(bool, usize)> {
    let mut quote: Option<char> = None;
    let mut start = 0usize;
    for (at, c) in chunk.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' => quote = Some(c),
                '[' | '{' => return None,
                ',' | ']' => {
                    let item = chunk.get(start..at)?.trim();
                    if !item.is_empty() {
                        items.push(item);
                    }
                    start = at.saturating_add(1);
                    if c == ']' {
                        return Some((true, start));
                    }
                }
                _ => {}
            },
        }
    }
    if quote.is_some() {
        return None;
    }
    let item = chunk.get(start..)?.trim();
    if !item.is_empty() {
        items.push(item);
    }
    Some((false, chunk.len()))
}

/// The items of a block sequence under a key at column `col`; `(items, next line)`.
fn block_items<'t>(
    col: usize,
    lines: &[&'t str],
    mut next: usize,
) -> Option<(Vec<&'t str>, usize)> {
    let mut items = Vec::new();
    while let Some(raw) = lines.get(next) {
        let line = strip_comment(raw);
        let body = line.trim_start_matches(' ');
        if body.trim().is_empty() {
            next = next.saturating_add(1);
            continue;
        }
        let indent = line.len().saturating_sub(body.len());
        if indent < col {
            break;
        }
        let Some(item) = body.strip_prefix('-') else {
            if indent == col {
                break;
            }
            // A continuation line or a nested construct this scanner does not follow.
            return None;
        };
        if !(item.is_empty() || item.starts_with(' ')) {
            if indent == col {
                break;
            }
            return None;
        }
        items.push(item.trim());
        next = next.saturating_add(1);
    }
    if items.is_empty() {
        // `stages:` with nothing below is `None`: `check_array` fails.
        return None;
    }
    Some((items, next))
}

/// The string value of a scalar item, or `None` when it is not a string (`check_string` fails) or beyond this
/// scanner.
fn scalar(item: &str) -> Option<String> {
    if let Some(inner) = item.strip_prefix('\'') {
        return Some(inner.strip_suffix('\'')?.replace("''", "'"));
    }
    if let Some(inner) = item.strip_prefix('"') {
        let inner = inner.strip_suffix('"')?;
        return (!inner.contains('\\')).then(|| inner.to_owned());
    }
    let first = item.chars().next()?;
    if "&*!{[|>?@`%,]".contains(first) || item.contains(": ") || item.contains(" #") {
        return None;
    }
    (!is_yaml11_non_string(item)).then(|| item.to_owned())
}

/// PyYAML's implicit resolvers (YAML 1.1) that turn a plain scalar into something other than `str`.
fn is_yaml11_non_string(s: &str) -> bool {
    const WORDS: [&str; 24] = [
        "~", "null", "Null", "NULL", "true", "True", "TRUE", "false", "False", "FALSE", "yes",
        "Yes", "YES", "no", "No", "NO", "on", "On", "ON", "off", "Off", "OFF", "y", "n",
    ];
    if s.is_empty() || WORDS.contains(&s) {
        // `y`/`n` are strings for PyYAML (its bool resolver omits them); they never name a stage, so treating them
        // as non-strings only matters for invalid manifests.
        return s != "y" && s != "n";
    }
    let digits = s.trim_start_matches(['+', '-']);
    let numeric = !digits.is_empty()
        && digits.bytes().all(|b| {
            b.is_ascii_digit() || b == b'_' || b == b'.' || b == b'e' || b == b'E' || b == b':'
        })
        && digits
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_digit() || b == b'.');
    numeric
        || digits.starts_with("0x")
        || digits.starts_with("0b")
        || matches!(digits, ".inf" | ".Inf" | ".INF" | ".nan" | ".NaN" | ".NAN")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_and_flow_sequences_in_first_seen_order() {
        let m = b"-   id: a\n    stages: [push, manual]\n-   id: b\n    stages:\n    - commit\n    -   'merge-commit'\n    - push\n-   id: c\n    entry: c\n";
        assert_eq!(legacy_stages(m), ["push", "commit", "merge-commit"]);
    }

    #[test]
    fn new_names_and_non_stage_keys_warn_nothing() {
        let m = b"-   id: a\n    stages: [pre-commit, pre-push]\n    name: commit push\n    args: [commit]\n";
        assert!(legacy_stages(m).is_empty());
    }

    #[test]
    fn invalid_minimal_manifest_warns_nothing() {
        // pre-commit swallows the InvalidManifestError of the minimal schema.
        assert!(legacy_stages(b"-   id: a\n    stages: commit\n").is_empty());
        assert!(legacy_stages(b"-   id: a\n    stages: [commit, 1]\n").is_empty());
        assert!(legacy_stages(b"-   id: a\n    stages: [commit, true]\n").is_empty());
        assert!(
            legacy_stages(b"-   id: a\n    stages:\n-   id: b\n    stages: [commit]\n").is_empty()
        );
        assert!(legacy_stages(b"\xff-   id: a\n    stages: [commit]\n").is_empty());
    }

    #[test]
    fn first_key_on_the_dash_line_and_multiline_flow() {
        let m = b"- stages: [\n    commit,  # old\n    push\n  ]\n  id: a\n";
        assert_eq!(legacy_stages(m), ["commit", "push"]);
        let m = b"-   stages:\n    - merge-commit\n    id: a\n";
        assert_eq!(legacy_stages(m), ["merge-commit"]);
    }

    #[test]
    fn quoted_keys_and_comments() {
        let m = b"-   id: a\n    'stages': [\"commit\"] # c\n    # stages: [push]\n";
        assert_eq!(legacy_stages(m), ["commit"]);
    }

    #[test]
    fn embedded_pre_v5_manifests_use_legacy_stages() {
        let p = snp_hooks_data::profile_for_rev("v4.4.0").unwrap();
        assert_eq!(
            legacy_stages(p.manifest_yaml.as_bytes()),
            ["commit", "push"]
        );
        let p = snp_hooks_data::profile_for_rev("v6.0.0").unwrap();
        assert!(legacy_stages(p.manifest_yaml.as_bytes()).is_empty());
    }

    #[test]
    fn yaml11_typing() {
        for s in [
            "1", "-2", "1.5", "1e3", "0x1f", ".inf", "null", "~", "Yes", "off", "1:30",
        ] {
            assert!(is_yaml11_non_string(s), "{s}");
        }
        for s in ["commit", "pre-commit", "y", "n", "e", "-", "1a"] {
            assert!(!is_yaml11_non_string(s), "{s}");
        }
    }
}
