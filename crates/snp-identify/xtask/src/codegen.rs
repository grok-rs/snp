//! Rendering of `src/generated.rs`.
//!
//! Lookup tables are emitted as sorted `(key, TagSet)` slices plus a
//! precomputed open-addressing index (FNV-1a + Fibonacci hashing, linear
//! probing). The hash functions here must stay byte-for-byte identical to
//! `src/table.rs`; the library's unit tests verify every generated key is
//! found through its index.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::{BASE_TAGS, PyTable, Tables, all_tags};

const EMPTY: u16 = u16::MAX;
const SEED_CANDIDATES: u64 = 4096;

/// FNV-1a over `key`, with the offset basis perturbed by `seed`.
fn hash(seed: u64, key: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64 ^ seed;
    for &b in key {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Fibonacci hashing of `h` into `64 - shift` bits.
fn slot(h: u64, shift: u32) -> usize {
    usize::try_from(h.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> shift).unwrap_or(0)
}

struct Index {
    seed: u64,
    shift: u32,
    max_probe: u32,
    min_len: usize,
    max_len: usize,
    slots: Vec<u16>,
}

fn build_index(keys: &[&str]) -> Result<Index, String> {
    let n = keys.len();
    if n >= usize::from(EMPTY) {
        return Err("too many keys for a u16 index".to_owned());
    }
    let size = (n * 2).next_power_of_two().max(8);
    let bits = size.trailing_zeros();
    let shift = 64 - bits;
    let mask = size - 1;
    let min_len = keys.iter().map(|k| k.len()).min().unwrap_or(0);
    let max_len = keys.iter().map(|k| k.len()).max().unwrap_or(0);
    let mut best: Option<(u32, u64, Index)> = None;
    for seed in 0..SEED_CANDIDATES {
        let mut slots = vec![EMPTY; size];
        let mut max_probe = 0u32;
        let mut total = 0u64;
        for (i, key) in keys.iter().enumerate() {
            let mut s = slot(hash(seed, key.as_bytes()), shift);
            let mut probe = 0u32;
            while slots.get(s).is_some_and(|&v| v != EMPTY) {
                s = (s + 1) & mask;
                probe += 1;
            }
            if let Some(v) = slots.get_mut(s) {
                *v = u16::try_from(i).map_err(|_| "index overflow".to_owned())?;
            }
            max_probe = max_probe.max(probe);
            total += u64::from(probe);
        }
        let better = best
            .as_ref()
            .is_none_or(|(bm, bt, _)| (max_probe, total) < (*bm, *bt));
        if better {
            best = Some((
                max_probe,
                total,
                Index {
                    seed,
                    shift,
                    max_probe,
                    min_len,
                    max_len,
                    slots,
                },
            ));
        }
        if max_probe == 0 {
            break;
        }
    }
    best.map(|(_, _, idx)| idx)
        .ok_or_else(|| "no index".to_owned())
}

fn render_index(out: &mut String, idx: &Index, indent: &str) {
    let _ = writeln!(
        out,
        "{indent}StrIndex::new({seed}, {shift}, {max_probe}, ({min_len}, {max_len}), &[",
        seed = idx.seed,
        shift = idx.shift,
        max_probe = idx.max_probe,
        min_len = idx.min_len,
        max_len = idx.max_len
    );
    for chunk in idx.slots.chunks(16) {
        let line: Vec<String> = chunk.iter().map(u16::to_string).collect();
        let _ = writeln!(out, "{indent}    {},", line.join(", "));
    }
    let _ = write!(out, "{indent}])");
}

fn rust_str(s: &str) -> String {
    format!("{s:?}")
}

fn render_table(
    out: &mut String,
    name: &str,
    doc: &str,
    table: &PyTable,
    ids: &BTreeMap<&str, usize>,
) -> Result<(), String> {
    let keys: Vec<&str> = table.keys().map(String::as_str).collect();
    let idx = build_index(&keys)?;
    out.push('\n');
    for line in doc.lines() {
        let _ = writeln!(out, "/// {line}");
    }
    let _ = writeln!(out, "pub static {name}: TagTable = TagTable::new(");
    let _ = writeln!(out, "    &[");
    for (key, tags) in table {
        let mut tag_ids: Vec<usize> = Vec::with_capacity(tags.len());
        for t in tags {
            tag_ids.push(
                *ids.get(t.as_str())
                    .ok_or_else(|| format!("unknown tag {t:?}"))?,
            );
        }
        tag_ids.sort_unstable();
        let id_list: Vec<String> = tag_ids.iter().map(usize::to_string).collect();
        let names: Vec<&str> = tags.iter().map(String::as_str).collect();
        let _ = writeln!(
            out,
            "        ({}, TagSet::from_ids(&[{}])), // {}",
            rust_str(key),
            id_list.join(", "),
            names.join(", ")
        );
    }
    let _ = writeln!(out, "    ],");
    render_index(out, &idx, "    ");
    let _ = writeln!(out, ",");
    let _ = writeln!(out, ");");
    Ok(())
}

fn base_tag_doc(tag: &str) -> &'static str {
    match tag {
        "directory" | "file" | "symlink" | "socket" => "file type tag (`identify.TYPE_TAGS`)",
        "executable" | "non-executable" => "file mode tag (`identify.MODE_TAGS`)",
        _ => "encoding tag (`identify.ENCODING_TAGS`)",
    }
}

/// Render the complete `src/generated.rs`.
#[allow(clippy::too_many_lines)]
pub fn render(t: &Tables) -> Result<String, String> {
    let all = all_tags(t);
    let ids: BTreeMap<&str, usize> = all
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();
    let words = all.len().div_ceil(64);
    let max_ext = t
        .extensions
        .keys()
        .chain(t.extensions_need_binary_check.keys())
        .map(String::len)
        .max()
        .unwrap_or(0);
    let v = &t.version;

    let mut out = String::new();
    let _ = writeln!(
        out,
        "// @generated by `cargo xtask gen-identify` from Python identify {v}."
    );
    let _ = writeln!(out, "// DO NOT EDIT. Regenerate with:");
    let _ = writeln!(
        out,
        "//     cargo xtask gen-identify --src <identify-{v}>/identify"
    );
    let _ = writeln!(
        out,
        "// identify/extensions.py and identify/interpreters.py are parsed as data"
    );
    let _ = writeln!(
        out,
        "// (xtask/src/pyliteral.rs); no Python code is executed."
    );
    out.push('\n');
    out.push_str("use crate::table::{StrIndex, TagTable};\n");
    out.push_str("use crate::tags::{Tag, TagSet};\n");
    out.push('\n');
    out.push_str("/// Version of the Python [identify](https://github.com/pre-commit/identify)\n");
    out.push_str("/// release the tables in this crate were generated from.\n");
    let _ = writeln!(out, "pub const IDENTIFY_VERSION: &str = {};", rust_str(v));
    out.push('\n');
    out.push_str("/// Number of tags known to identify (`len(identify.ALL_TAGS)`).\n");
    let _ = writeln!(out, "pub const TAG_COUNT: usize = {};", all.len());
    out.push('\n');
    out.push_str("/// Number of `u64` words in a [`TagSet`] bitset.\n");
    let _ = writeln!(out, "pub(crate) const WORDS: usize = {words};");
    out.push('\n');
    out.push_str("/// Length in bytes of the longest extension key.\n");
    let _ = writeln!(
        out,
        "pub(crate) const MAX_EXTENSION_LEN: usize = {max_ext};"
    );
    out.push('\n');
    out.push_str("/// Every tag known to identify (`identify.ALL_TAGS`), sorted by name.\n");
    out.push_str("///\n");
    out.push_str("/// A tag's position in this array is its [`Tag::id`].\n");
    out.push_str("pub static ALL_TAGS: [&str; TAG_COUNT] = [\n");
    for (i, tag) in all.iter().enumerate() {
        let _ = writeln!(out, "    {}, // {i}", rust_str(tag));
    }
    out.push_str("];\n");
    out.push('\n');
    out.push_str("/// Index from tag name to position in [`ALL_TAGS`].\n");
    out.push_str("pub(crate) static TAG_INDEX: StrIndex =\n");
    let names: Vec<&str> = all.iter().map(String::as_str).collect();
    render_index(&mut out, &build_index(&names)?, "    ");
    out.push_str(";\n");
    out.push('\n');
    out.push_str("impl Tag {\n");
    let mut base: Vec<&str> = BASE_TAGS.to_vec();
    base.sort_unstable();
    for (n, tag) in base.iter().enumerate() {
        let id = ids
            .get(tag)
            .ok_or_else(|| format!("missing base tag {tag}"))?;
        if n > 0 {
            out.push('\n');
        }
        let _ = writeln!(out, "    /// `{tag}`: {}.", base_tag_doc(tag));
        let _ = writeln!(
            out,
            "    pub const {}: Tag = Tag::from_id_const({id});",
            tag.to_ascii_uppercase().replace('-', "_")
        );
    }
    out.push_str("}\n");

    render_table(
        &mut out,
        "EXTENSIONS",
        "`identify.extensions.EXTENSIONS`: lower-cased file extension (without the\n\
         leading dot) → tags. Every entry carries exactly one of `text` / `binary`.",
        &t.extensions,
        &ids,
    )?;
    render_table(
        &mut out,
        "EXTENSIONS_NEED_BINARY_CHECK",
        "`identify.extensions.EXTENSIONS_NEED_BINARY_CHECK`: extensions whose files\n\
         may be text or binary; the encoding tag is decided from the content.",
        &t.extensions_need_binary_check,
        &ids,
    )?;
    render_table(
        &mut out,
        "NAMES",
        "`identify.extensions.NAMES`: exact file names (or dot-separated name parts,\n\
         e.g. `Dockerfile.xenial`) → tags. Matching is case-sensitive.",
        &t.names,
        &ids,
    )?;
    render_table(
        &mut out,
        "INTERPRETERS",
        "`identify.interpreters.INTERPRETERS`: shebang interpreter name → tags.",
        &t.interpreters,
        &ids,
    )?;
    Ok(out)
}

/// Recover the tables from a previously generated file (for drift reports).
pub fn parse_back(src: &str) -> BTreeMap<String, PyTable> {
    let mut out: BTreeMap<String, PyTable> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in src.lines() {
        if let Some(rest) = line.strip_prefix("pub static ") {
            current = rest
                .split_once(": TagTable")
                .map(|(name, _)| name.to_owned());
            continue;
        }
        let Some(name) = current.as_ref() else {
            continue;
        };
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("(\"") else {
            continue;
        };
        let Some((key, rest)) = rest.split_once("\", TagSet") else {
            continue;
        };
        let Some((_, tags)) = rest.split_once("// ") else {
            continue;
        };
        let key = key.replace("\\\"", "\"").replace("\\\\", "\\");
        let tags: BTreeSet<String> = tags.split(", ").map(str::to_owned).collect();
        out.entry(name.clone()).or_default().insert(key, tags);
    }
    out
}
