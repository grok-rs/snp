//! lefthook's configuration data model before typing: koanf's `map[string]any` tree, read from YAML (go-yaml v3,
//! YAML 1.2 booleans), JSON, JSONC and TOML, plus mapstructure's `WeaklyTypedInput` conversions (05 §2.2).
//!
//! Owner: WP-34.

use indexmap::IndexMap;
use std::fmt::Write as _;

/// A koanf map: string keys in document order (Go maps are unordered; document order keeps the output stable).
pub(crate) type LMap = IndexMap<String, LValue>;

/// One configuration value as koanf holds it after parsing.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LValue {
    /// `null` / `~`.
    Null,
    /// `true` / `false` (YAML 1.2: only these two spellings).
    Bool(bool),
    /// An integer.
    Int(i64),
    /// A float.
    Float(f64),
    /// A string (also a TOML date-time or a YAML integer beyond `i64`: koanf hands them on as text).
    Str(String),
    /// A list.
    List(Vec<Self>),
    /// A map with string keys.
    Map(LMap),
}

impl LValue {
    /// The Go-ish type name used in conversion errors.
    pub(crate) const fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Str(_) => "string",
            Self::List(_) => "list",
            Self::Map(_) => "map",
        }
    }

    /// The map, if this is one.
    pub(crate) const fn as_map(&self) -> Option<&LMap> {
        match self {
            Self::Map(m) => Some(m),
            _ => None,
        }
    }

    /// The string, if this is one.
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// The source format of one configuration file, chosen by its extension (`loader.go:38-51`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Format {
    /// `.yml` / `.yaml`.
    Yaml,
    /// `.json`.
    Json,
    /// `.jsonc` (comments and trailing commas stripped first).
    Jsonc,
    /// `.toml`.
    Toml,
}

impl Format {
    /// The format of a path, `None` for an unsupported extension (`parsers[filepath.Ext(path)]`).
    pub(crate) fn of(path: &std::path::Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "yml" | "yaml" => Some(Self::Yaml),
            "json" => Some(Self::Json),
            "jsonc" => Some(Self::Jsonc),
            "toml" => Some(Self::Toml),
            _ => None,
        }
    }
}

/// The deepest nesting a configuration may have (koanf has no limit; this one bounds every recursion).
pub(crate) const MAX_DEPTH: usize = 64;

/// The largest configuration file read (lefthook has no limit).
pub(crate) const MAX_BYTES: usize = 4 << 20;

/// Parses one configuration file. The error text carries the parser's line and column.
pub(crate) fn parse(text: &str, format: Format) -> Result<LMap, String> {
    if text.len() > MAX_BYTES {
        return Err(format!(
            "the file is {} bytes long, more than the limit of {MAX_BYTES} bytes",
            text.len()
        ));
    }
    let value = match format {
        Format::Yaml => from_yaml(text)?,
        Format::Json => from_json(text)?,
        Format::Jsonc => from_json(&jsonc_to_json(text))?,
        Format::Toml => from_toml(text)?,
    };
    match value {
        LValue::Map(m) => Ok(m),
        // An empty YAML document is an empty config.
        LValue::Null => Ok(LMap::new()),
        other => Err(format!(
            "expected a mapping at the top level, got a {}",
            other.kind()
        )),
    }
}

/// The most bytes a YAML document may expand to once every alias is copied (twice the input limit).
const MAX_EXPANDED: usize = MAX_BYTES.saturating_mul(2);

/// The size of `doc` with every alias expanded: scalar bytes plus one per node, counted at every use. The node limit
/// of the constructor counts nodes, not bytes, so 5 000 aliases of one 3 MiB scalar would otherwise be copied in full
/// (the DAG is walked once, iteratively; a cycle counts as nothing and is left to the constructor's error).
fn expanded_size(doc: &snp_yaml::Document) -> usize {
    let Some(root) = doc.root else {
        return 0;
    };
    let idx = |id: snp_yaml::NodeId| usize::try_from(id.0).unwrap_or(usize::MAX);
    let n = doc.nodes.len();
    let mut size: Vec<Option<usize>> = vec![None; n];
    let mut open = vec![false; n];
    let children = |i: usize| -> Vec<usize> {
        match doc.nodes.get(i).map(|node| &node.kind) {
            Some(snp_yaml::NodeKind::Seq(items)) => items.iter().map(|c| idx(*c)).collect(),
            Some(snp_yaml::NodeKind::Map(pairs)) => {
                pairs.iter().flat_map(|(k, v)| [idx(*k), idx(*v)]).collect()
            }
            _ => Vec::new(),
        }
    };
    let mut stack = vec![(idx(root), false)];
    while let Some((i, done)) = stack.pop() {
        if size.get(i).copied().flatten().is_some() {
            continue;
        }
        if let Some(snp_yaml::NodeKind::Scalar(text)) = doc.nodes.get(i).map(|node| &node.kind) {
            if let Some(slot) = size.get_mut(i) {
                *slot = Some(text.len().saturating_add(1));
            }
            continue;
        }
        if done {
            let total = children(i).into_iter().fold(1_usize, |acc, c| {
                acc.saturating_add(size.get(c).copied().flatten().unwrap_or(0))
            });
            if let Some(slot) = size.get_mut(i) {
                *slot = Some(total);
            }
            if let Some(o) = open.get_mut(i) {
                *o = false;
            }
            continue;
        }
        if open.get(i).copied().unwrap_or(true) {
            continue;
        }
        if let Some(o) = open.get_mut(i) {
            *o = true;
        }
        stack.push((i, true));
        for c in children(i) {
            if size.get(c).copied().flatten().is_none() && !open.get(c).copied().unwrap_or(true) {
                stack.push((c, false));
            }
        }
    }
    size.get(idx(root)).copied().flatten().unwrap_or(0)
}

fn too_deep() -> String {
    format!("the configuration is nested deeper than {MAX_DEPTH} levels")
}

/// go-yaml v3 through koanf: the YAML 1.2 core schema of snp-yaml (`Schema::Yaml12Core`), map keys turned into
/// strings with Go's `%v` (`maps.IntfaceKeysToStrings`).
#[expect(
    clippy::redundant_closure_for_method_calls,
    reason = "the method paths name cfgv::Value, which snp-migrate has no edge to"
)]
fn from_yaml(text: &str) -> Result<LValue, String> {
    let limits = snp_yaml::Limits {
        max_depth: 128,
        max_constructed_nodes: 200_000,
        max_bytes: MAX_BYTES,
    };
    let doc = snp_yaml::compose(text, snp_yaml::Schema::Yaml12Core, &limits)
        .map_err(|e| e.to_string())?;
    if expanded_size(&doc) > MAX_EXPANDED {
        return Err(format!(
            "the configuration expands (through aliases) to more than {MAX_EXPANDED} bytes"
        ));
    }
    let v = snp_yaml::construct(&doc, snp_yaml::Schema::Yaml12Core, &limits)
        .map_err(|e| e.to_string())?;
    // snp-migrate has no direct cfgv edge (ARCHITECTURE §1.2): the constructed value is used through its methods,
    // which the closures reach with the type inferred from `v` (closure arguments are checked last).
    let ops = Ops::new(
        &v,
        |v| v.type_name(),
        |v| v.as_bool().unwrap_or(false),
        |v| v.as_int(),
        |v| v.as_float().unwrap_or(0.0),
        |v| match v.as_str() {
            Some(s) => s.to_owned(),
            None => v.try_to_string().unwrap_or_default(),
        },
        |v| match v.as_array() {
            Some(items) => items.iter().collect(),
            None => v.as_set().map_or_default(|s| s.iter().collect()),
        },
        |v| v.as_map().map_or_default(|m| m.iter().collect()),
    );
    let mut budget = 400_000_usize;
    from_constructed(&v, &ops, 0, &mut budget)
}

/// The operations of snp-yaml's constructed value this module needs.
struct Ops<V> {
    /// Python type name.
    kind: fn(&V) -> &'static str,
    /// The bool.
    boolean: fn(&V) -> bool,
    /// The int, when it fits an `i64`.
    int: fn(&V) -> Option<i64>,
    /// The float.
    float: fn(&V) -> f64,
    /// The string (or `str()` of anything else).
    text: fn(&V) -> String,
    /// The items of a list, tuple or set.
    items: fn(&V) -> Vec<&V>,
    /// The pairs of a dict.
    pairs: fn(&V) -> Vec<(&V, &V)>,
}

/// `fn(&V) -> Vec<(&V, &V)>`.
type PairsFn<V> = fn(&V) -> Vec<(&V, &V)>;

impl<V> Ops<V> {
    #[expect(
        clippy::too_many_arguments,
        reason = "one function pointer per operation"
    )]
    fn new(
        _probe: &V,
        kind: fn(&V) -> &'static str,
        boolean: fn(&V) -> bool,
        int: fn(&V) -> Option<i64>,
        float: fn(&V) -> f64,
        text: fn(&V) -> String,
        items: fn(&V) -> Vec<&V>,
        pairs: PairsFn<V>,
    ) -> Self {
        Self {
            kind,
            boolean,
            int,
            float,
            text,
            items,
            pairs,
        }
    }
}

/// Converts snp-yaml's constructed value into an [`LValue`] (bounded depth and size).
fn from_constructed<V>(
    v: &V,
    ops: &Ops<V>,
    depth: usize,
    budget: &mut usize,
) -> Result<LValue, String> {
    if depth > MAX_DEPTH {
        return Err(too_deep());
    }
    *budget = budget
        .checked_sub(1)
        .ok_or_else(|| "the configuration has too many values".to_owned())?;
    let next = depth.saturating_add(1);
    Ok(match (ops.kind)(v) {
        "NoneType" => LValue::Null,
        "bool" => LValue::Bool((ops.boolean)(v)),
        "int" => (ops.int)(v).map_or_else(|| LValue::Str((ops.text)(v)), LValue::Int),
        "float" => LValue::Float((ops.float)(v)),
        "list" | "tuple" | "set" => {
            let mut out = Vec::new();
            for item in (ops.items)(v) {
                out.push(from_constructed(item, ops, next, budget)?);
            }
            LValue::List(out)
        }
        "dict" => {
            let mut out = LMap::new();
            for (k, val) in (ops.pairs)(v) {
                let key = match (ops.kind)(k) {
                    "NoneType" => "<nil>".to_owned(),
                    "bool" => (ops.boolean)(k).to_string(),
                    "float" => go_format_float((ops.float)(k)),
                    _ => (ops.text)(k),
                };
                let val = from_constructed(val, ops, next, budget)?;
                out.insert(key, val);
            }
            LValue::Map(out)
        }
        _ => LValue::Str((ops.text)(v)),
    })
}

/// encoding/json through koanf (`preserve_order` keeps the document order).
fn from_json(text: &str) -> Result<LValue, String> {
    // serde_json has its own recursion limit (128) that turns deep input into an error, never a stack overflow.
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    from_json_value(&v, 0)
}

fn from_json_value(v: &serde_json::Value, depth: usize) -> Result<LValue, String> {
    if depth > MAX_DEPTH {
        return Err(too_deep());
    }
    let next = depth.saturating_add(1);
    Ok(match v {
        serde_json::Value::Null => LValue::Null,
        serde_json::Value::Bool(b) => LValue::Bool(*b),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map_or_else(|| LValue::Float(n.as_f64().unwrap_or(0.0)), LValue::Int),
        serde_json::Value::String(s) => LValue::Str(s.clone()),
        serde_json::Value::Array(items) => LValue::List(
            items
                .iter()
                .map(|i| from_json_value(i, next))
                .collect::<Result<_, _>>()?,
        ),
        serde_json::Value::Object(m) => {
            let mut out = LMap::new();
            for (k, v) in m {
                out.insert(k.clone(), from_json_value(v, next)?);
            }
            LValue::Map(out)
        }
    })
}

/// `tidwall/jsonc.ToJSON`: `//` and `/* */` comments become spaces (line breaks kept, so error positions stay
/// right) and a `,` followed only by whitespace before `}` / `]` is dropped. String contents are untouched.
pub(crate) fn jsonc_to_json(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::new();
    let mut i = 0_usize;
    let at = |i: usize| chars.get(i).copied();
    while let Some(c) = at(i) {
        match c {
            '"' => {
                out.push(c);
                i = i.saturating_add(1);
                while let Some(d) = at(i) {
                    out.push(d);
                    i = i.saturating_add(1);
                    if d == '\\' {
                        if let Some(e) = at(i) {
                            out.push(e);
                            i = i.saturating_add(1);
                        }
                    } else if d == '"' {
                        break;
                    }
                }
            }
            '/' if at(i.saturating_add(1)) == Some('/') => {
                while let Some(d) = at(i) {
                    if d == '\n' {
                        break;
                    }
                    out.push(' ');
                    i = i.saturating_add(1);
                }
            }
            '/' if at(i.saturating_add(1)) == Some('*') => {
                out.push_str("  ");
                i = i.saturating_add(2);
                loop {
                    match at(i) {
                        None => break,
                        Some('*') if at(i.saturating_add(1)) == Some('/') => {
                            out.push_str("  ");
                            i = i.saturating_add(2);
                            break;
                        }
                        Some(d) => {
                            out.push(if d == '\n' { '\n' } else { ' ' });
                            i = i.saturating_add(1);
                        }
                    }
                }
            }
            ',' => {
                // Trailing comma: the next significant character (skipping whitespace and comments) closes.
                let mut j = i.saturating_add(1);
                let closes = loop {
                    match at(j) {
                        Some(d) if d.is_whitespace() => j = j.saturating_add(1),
                        Some('/') if at(j.saturating_add(1)) == Some('/') => {
                            while at(j).is_some_and(|d| d != '\n') {
                                j = j.saturating_add(1);
                            }
                        }
                        Some('/') if at(j.saturating_add(1)) == Some('*') => {
                            j = j.saturating_add(2);
                            while at(j).is_some()
                                && !(at(j) == Some('*') && at(j.saturating_add(1)) == Some('/'))
                            {
                                j = j.saturating_add(1);
                            }
                            j = j.saturating_add(2);
                        }
                        Some('}' | ']') => break true,
                        _ => break false,
                    }
                };
                out.push(if closes { ' ' } else { ',' });
                i = i.saturating_add(1);
            }
            _ => {
                out.push(c);
                i = i.saturating_add(1);
            }
        }
    }
    out
}

/// go-toml v2 through koanf: date-times become their text.
fn from_toml(text: &str) -> Result<LValue, String> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    from_toml_table(&table, 0)
}

fn from_toml_table(t: &toml::Table, depth: usize) -> Result<LValue, String> {
    if depth > MAX_DEPTH {
        return Err(too_deep());
    }
    let mut out = LMap::new();
    for (k, v) in t {
        out.insert(k.clone(), from_toml_value(v, depth.saturating_add(1))?);
    }
    Ok(LValue::Map(out))
}

fn from_toml_value(v: &toml::Value, depth: usize) -> Result<LValue, String> {
    if depth > MAX_DEPTH {
        return Err(too_deep());
    }
    Ok(match v {
        toml::Value::String(s) => LValue::Str(s.clone()),
        toml::Value::Integer(i) => LValue::Int(*i),
        toml::Value::Float(f) => LValue::Float(*f),
        toml::Value::Boolean(b) => LValue::Bool(*b),
        toml::Value::Datetime(d) => LValue::Str(d.to_string()),
        toml::Value::Array(items) => LValue::List(
            items
                .iter()
                .map(|i| from_toml_value(i, depth.saturating_add(1)))
                .collect::<Result<_, _>>()?,
        ),
        toml::Value::Table(t) => from_toml_table(t, depth)?,
    })
}

/// Go's `strconv.FormatFloat(f, 'f', -1, 64)`: the shortest decimal that round-trips, never an exponent.
pub(crate) fn go_format_float(f: f64) -> String {
    if f.is_nan() {
        "NaN".to_owned()
    } else if f.is_infinite() {
        if f > 0.0 { "+Inf" } else { "-Inf" }.to_owned()
    } else {
        // Rust's `Display` for f64 is the shortest round-trip representation without an exponent.
        format!("{f}")
    }
}

/// A mapstructure decoding error: the dotted key path and what was wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypeError {
    /// `pre-commit.jobs[0].run`.
    pub(crate) path: String,
    /// `expected a string, got a list`.
    pub(crate) msg: String,
}

impl TypeError {
    pub(crate) fn new(path: &str, want: &str, got: &LValue) -> Self {
        Self {
            path: path.to_owned(),
            msg: format!("expected {want}, got {}", describe(got)),
        }
    }
}

/// `a list`, `the string 'yes'`, …
fn describe(v: &LValue) -> String {
    match v {
        LValue::Str(s) => format!("the string {}", go_quote(s)),
        LValue::Int(i) => format!("the int {i}"),
        LValue::Float(f) => format!("the float {}", go_format_float(*f)),
        LValue::Bool(b) => format!("the bool {b}"),
        other => format!("a {}", other.kind()),
    }
}

/// `%q`-like quoting for messages (backslash escapes for `"`, `\` and control characters).
pub(crate) fn go_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => {
                write!(out, "\\u{:04x}", u32::from(c)).unwrap_or_default();
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// mapstructure weak `string`: bools are `1`/`0`, numbers base 10, `null` is `""`.
pub(crate) fn weak_string(v: &LValue, path: &str) -> Result<String, TypeError> {
    match v {
        LValue::Null => Ok(String::new()),
        LValue::Str(s) => Ok(s.clone()),
        LValue::Bool(b) => Ok(if *b { "1" } else { "0" }.to_owned()),
        LValue::Int(i) => Ok(i.to_string()),
        LValue::Float(f) => Ok(go_format_float(*f)),
        other => Err(TypeError::new(path, "a string", other)),
    }
}

/// mapstructure weak `bool`: numbers are `!= 0`, strings go through `strconv.ParseBool` (`""` is false).
pub(crate) fn weak_bool(v: &LValue, path: &str) -> Result<bool, TypeError> {
    match v {
        LValue::Null => Ok(false),
        LValue::Bool(b) => Ok(*b),
        LValue::Int(i) => Ok(*i != 0),
        LValue::Float(f) => Ok(*f != 0.0),
        LValue::Str(s) => match s.as_str() {
            "1" | "t" | "T" | "TRUE" | "true" | "True" => Ok(true),
            "0" | "f" | "F" | "FALSE" | "false" | "False" | "" => Ok(false),
            _ => Err(TypeError::new(path, "a bool", v)),
        },
        other => Err(TypeError::new(path, "a bool", other)),
    }
}

/// mapstructure weak `int`: bools are 1/0, floats truncate, strings use `strconv.ParseInt(s, 0, 64)` (`""` is 0).
pub(crate) fn weak_int(v: &LValue, path: &str) -> Result<i64, TypeError> {
    match v {
        LValue::Null => Ok(0),
        LValue::Bool(b) => Ok(i64::from(*b)),
        LValue::Int(i) => Ok(*i),
        LValue::Float(f) if f.is_finite() && f.abs() < 9.2e18 => Ok(trunc_i64(*f)),
        LValue::Str(s) => go_parse_int(s).ok_or_else(|| TypeError::new(path, "an int", v)),
        other => Err(TypeError::new(path, "an int", other)),
    }
}

/// `int64(f)` of a finite float inside the `i64` range.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "Go's int64(f) truncation; the caller bounds |f| below 2^63"
)]
const fn trunc_i64(f: f64) -> i64 {
    f as i64
}

/// `strconv.ParseInt(s, 0, 64)`: optional sign, `0x`/`0o`/`0b`/leading-`0` prefixes, `_` between digits only with a
/// prefix-implied base.
pub(crate) fn go_parse_int(s: &str) -> Option<i64> {
    let s = if s.is_empty() { "0" } else { s };
    let (neg, body) = match s.as_bytes().first() {
        Some(b'-') => (true, s.get(1..)?),
        Some(b'+') => (false, s.get(1..)?),
        _ => (false, s),
    };
    let lower = body.to_ascii_lowercase();
    let (radix, digits, prefixed) = if let Some(d) = lower.strip_prefix("0x") {
        (16, d.to_owned(), true)
    } else if let Some(d) = lower.strip_prefix("0o") {
        (8, d.to_owned(), true)
    } else if let Some(d) = lower.strip_prefix("0b") {
        (2, d.to_owned(), true)
    } else if lower.len() > 1 && lower.starts_with('0') {
        (8, lower.get(1..)?.to_owned(), true)
    } else {
        (10, lower, false)
    };
    if digits.contains('_') {
        // underscoreOK: only with a base prefix, never doubled, leading or trailing.
        if !prefixed || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
            return None;
        }
    }
    let digits: String = digits.chars().filter(|c| *c != '_').collect();
    if digits.is_empty() {
        return None;
    }
    let magnitude = u64::from_str_radix(&digits, radix).ok()?;
    if neg {
        if magnitude == 1_u64 << 63 {
            Some(i64::MIN)
        } else {
            i64::try_from(magnitude).ok().map(i64::wrapping_neg)
        }
    } else {
        i64::try_from(magnitude).ok()
    }
}

/// mapstructure weak `[]string`: a scalar becomes a one-element list, `null` and an empty map are empty.
pub(crate) fn weak_str_list(v: &LValue, path: &str) -> Result<Vec<String>, TypeError> {
    match v {
        LValue::Null => Ok(Vec::new()),
        LValue::List(items) => items
            .iter()
            .enumerate()
            .map(|(i, item)| weak_string(item, &format!("{path}[{i}]")))
            .collect(),
        LValue::Map(m) if m.is_empty() => Ok(Vec::new()),
        LValue::Map(_) => Err(TypeError::new(path, "a list of strings", v)),
        scalar => Ok(vec![weak_string(scalar, path)?]),
    }
}

/// mapstructure weak `map[string]string`: values weakly converted, a list of maps merged, an empty list empty.
pub(crate) fn weak_str_map(v: &LValue, path: &str) -> Result<Vec<(String, String)>, TypeError> {
    let mut out: IndexMap<String, String> = IndexMap::new();
    let mut add = |m: &LMap| -> Result<(), TypeError> {
        for (k, val) in m {
            out.insert(k.clone(), weak_string(val, &format!("{path}.{k}"))?);
        }
        Ok(())
    };
    match v {
        LValue::Null => {}
        LValue::Map(m) => add(m)?,
        LValue::List(items) => {
            for item in items {
                match item {
                    LValue::Map(m) => add(m)?,
                    other => return Err(TypeError::new(path, "a map of strings", other)),
                }
            }
        }
        other => return Err(TypeError::new(path, "a map of strings", other)),
    }
    Ok(out.into_iter().collect())
}

/// mapstructure field lookup: the exact key first, else the first key equal under Unicode case folding
/// (`strings.EqualFold`).
pub(crate) fn field<'a>(m: &'a LMap, name: &str) -> Option<&'a LValue> {
    m.get(name)
        .or_else(|| m.iter().find(|(k, _)| eq_fold(k, name)).map(|(_, v)| v))
}

/// `strings.EqualFold` (simple Unicode case folding approximated by lowercase comparison per char).
fn eq_fold(left: &str, right: &str) -> bool {
    left.chars()
        .flat_map(char::to_lowercase)
        .eq(right.chars().flat_map(char::to_lowercase))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml12_booleans() {
        let m = parse("a: yes\nb: true\nc: on\nd: 3\ne: 1.5\nf: ~\n", Format::Yaml).unwrap();
        assert_eq!(m["a"], LValue::Str("yes".into()));
        assert_eq!(m["b"], LValue::Bool(true));
        assert_eq!(m["c"], LValue::Str("on".into()));
        assert_eq!(m["d"], LValue::Int(3));
        assert_eq!(m["e"], LValue::Float(1.5));
        assert_eq!(m["f"], LValue::Null);
    }

    #[test]
    fn yaml_keys_become_strings() {
        let m = parse("2: a\ntrue: b\n", Format::Yaml).unwrap();
        assert!(m.contains_key("2"));
        assert!(m.contains_key("true"));
    }

    #[test]
    fn jsonc_comments_and_trailing_commas() {
        let src = "{\n  // c\n  \"a\": [1, 2,], /* x */ \"b\": \"//not\",\n}\n";
        let m = parse(src, Format::Jsonc).unwrap();
        assert_eq!(m["a"], LValue::List(vec![LValue::Int(1), LValue::Int(2)]));
        assert_eq!(m["b"], LValue::Str("//not".into()));
        assert_eq!(jsonc_to_json("[1,\n// c\n]"), "[1 \n    \n]");
    }

    #[test]
    fn toml_tables() {
        let m = parse("[pre-commit.commands.lint]\nrun = \"x\"\n", Format::Toml).unwrap();
        let pc = m["pre-commit"].as_map().unwrap();
        assert!(pc["commands"].as_map().unwrap().contains_key("lint"));
    }

    #[test]
    fn weak_typing() {
        assert_eq!(weak_string(&LValue::Int(3), "x").unwrap(), "3");
        assert_eq!(weak_string(&LValue::Bool(true), "x").unwrap(), "1");
        assert_eq!(weak_string(&LValue::Float(1.5), "x").unwrap(), "1.5");
        assert!(weak_bool(&LValue::Str("true".into()), "x").unwrap());
        assert!(!weak_bool(&LValue::Str(String::new()), "x").unwrap());
        assert!(weak_bool(&LValue::Str("yes".into()), "x").is_err());
        assert_eq!(weak_int(&LValue::Str("0x10".into()), "x").unwrap(), 16);
        assert_eq!(weak_int(&LValue::Str("010".into()), "x").unwrap(), 8);
        assert_eq!(weak_int(&LValue::Str("-5".into()), "x").unwrap(), -5);
        assert!(weak_int(&LValue::Str("1_0".into()), "x").is_err());
        assert_eq!(weak_int(&LValue::Str("0x1_0".into()), "x").unwrap(), 16);
        assert_eq!(
            weak_str_list(&LValue::Str("*.js".into()), "x").unwrap(),
            vec!["*.js".to_owned()]
        );
        assert_eq!(
            weak_str_list(&LValue::Int(3), "x").unwrap(),
            vec!["3".to_owned()]
        );
        let e = weak_bool(&LValue::Str("yes".into()), "pre-commit.parallel").unwrap_err();
        assert_eq!(e.msg, "expected a bool, got the string \"yes\"");
    }

    #[test]
    fn go_ints() {
        assert_eq!(go_parse_int("9223372036854775807"), Some(i64::MAX));
        assert_eq!(go_parse_int("-9223372036854775808"), Some(i64::MIN));
        assert_eq!(go_parse_int("9223372036854775808"), None);
        assert_eq!(go_parse_int("0b101"), Some(5));
        assert_eq!(go_parse_int("x"), None);
        assert_eq!(go_parse_int(""), Some(0));
    }

    #[test]
    fn field_lookup_is_case_insensitive() {
        let m = parse("Run: x\n", Format::Yaml).unwrap();
        assert_eq!(field(&m, "run"), Some(&LValue::Str("x".into())));
    }

    #[test]
    fn floats_format_like_go() {
        assert_eq!(go_format_float(3.0), "3");
        assert_eq!(go_format_float(0.1), "0.1");
        assert_eq!(go_format_float(1e21), "1000000000000000000000");
    }
}
