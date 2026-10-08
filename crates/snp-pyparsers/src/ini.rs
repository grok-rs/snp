//! The `configparser` subset `detect-aws-credentials` uses (09 §4.D; `detect_aws_credentials.py`
//! `get_aws_secrets_from_file`): `ConfigParser()` with its defaults — delimiters `=` and `:`, full-line comments
//! `#`/`;`, no inline comments, `strict`, empty lines inside values, `optionxform = str.lower`, the `DEFAULT`
//! section, `BasicInterpolation` — reading one file, then `parser.get(section, var).strip()` for the three AWS
//! secret options of every section. Ported from `Lib/configparser.py` (3.12 and 3.13 agree for this
//! configuration; the vectors are generated under both).
//!
//! Owner: WP-17.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use snp_pycompat::chars::is_space;
use snp_pycompat::repr::repr_str;
use snp_pycompat::text::{rstrip, strip};

use crate::CheckError;

/// The options `get_aws_secrets_from_file` reads, in its order.
const AWS_VARS: [&str; 3] = [
    "aws_secret_access_key",
    "aws_security_token",
    "aws_session_token",
];

/// `MAX_INTERPOLATION_DEPTH`.
const MAX_INTERPOLATION_DEPTH: usize = 10;

/// `configparser.RawConfigParser().read_string(text)`: every `(option, value)` pair in file order; `Err` is the
/// `configparser.Error` text.
///
/// As `detect-aws-credentials` reads a credential file (source name `'<string>'`): the `(option, value)` pairs
/// `get_aws_secrets_from_file` collects — for every section in file order (not `DEFAULT`, whose values sections
/// inherit), each of `aws_secret_access_key`, `aws_security_token`, `aws_session_token` that resolves, its
/// interpolated value `.strip()`ped, when non-empty. A file without a section header gives no pairs (the hook
/// catches `MissingSectionHeaderError`); any other `configparser.Error` is the hook's crash and `Err` carries its
/// traceback line (`configparser.DuplicateSectionError: While reading from …`). See [`aws_credentials_ex`].
pub fn aws_credentials(text: &str) -> Result<Vec<(String, String)>, String> {
    aws_credentials_ex(text, "<string>").map_err(CheckError::into_message)
}

/// [`aws_credentials`] for a file read as `parser.read(source)` (ARCHITECTURE §1.4.11 addition): `text` is its
/// decoded content (universal newlines are applied here), `source` the path as passed to `read()` (it appears in
/// error texts). `Err` is always the crash path (`CheckError::Crash` with the module-qualified class).
pub fn aws_credentials_ex(text: &str, source: &str) -> Result<Vec<(String, String)>, CheckError> {
    let parsed = match read(text, source) {
        Ok(p) => p,
        Err(ReadError::MissingSectionHeader) => return Ok(Vec::new()),
        Err(ReadError::Crash(e)) => return Err(e),
    };
    let mut out = Vec::new();
    for (name, _) in &parsed.sections {
        for var in AWS_VARS {
            let Some(raw) = parsed.lookup(name, var) else {
                continue;
            };
            let value = interpolate(&parsed, name, var, raw)?;
            let key = strip(&value);
            if !key.is_empty() {
                out.push((var.to_owned(), key.to_owned()));
            }
        }
    }
    Ok(out)
}

/// One section: option → joined value, in insertion order.
#[derive(Debug, Default)]
struct Section {
    options: Vec<(String, String)>,
    index: HashMap<String, usize>,
}

impl Section {
    fn get(&self, option: &str) -> Option<&str> {
        self.index
            .get(option)
            .and_then(|&i| self.options.get(i))
            .map(|(_, v)| v.as_str())
    }
}

/// The parsed file.
#[derive(Debug, Default)]
struct Parsed {
    defaults: Section,
    sections: Vec<(String, Section)>,
}

impl Parsed {
    fn section(&self, name: &str) -> Option<&Section> {
        self.sections
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s)
    }

    /// `ChainMap(section, defaults)[option]`.
    fn lookup(&self, section: &str, option: &str) -> Option<&str> {
        self.section(section)
            .and_then(|s| s.get(option))
            .or_else(|| self.defaults.get(option))
    }
}

enum ReadError {
    MissingSectionHeader,
    Crash(CheckError),
}

fn crash(ty: &'static str, text: String) -> ReadError {
    ReadError::Crash(CheckError::crash(ty, text))
}

/// Options of one section while reading: name → value lines (joined at the end).
type RawOptions = Vec<(String, Vec<String>)>;

/// Which section the parser is filling.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Cur {
    None,
    Default,
    Section(usize),
}

/// `RawConfigParser._read` + `_join_multiline_values` (values are lists of lines until joined).
fn read(text: &str, source: &str) -> Result<Parsed, ReadError> {
    let text = snp_pycompat::textio::universal_newlines(text);
    let mut defaults: RawOptions = Vec::new();
    let mut sections: Vec<(String, RawOptions)> = Vec::new();
    let mut section_index: HashMap<String, usize> = HashMap::new();
    let mut added_sections: HashSet<String> = HashSet::new();
    let mut added_options: HashSet<(String, String)> = HashSet::new();
    let mut cur = Cur::None;
    let mut sectname = String::new();
    let mut optname: Option<String> = None;
    let mut indent_level = 0usize;
    let mut errors: Vec<(usize, &str)> = Vec::new();

    for (line, lineno) in text.split_inclusive('\n').zip(1usize..) {
        let stripped = strip(line);
        let clean = if stripped.starts_with(['#', ';']) {
            ""
        } else {
            stripped
        };
        if clean.is_empty() {
            // empty_lines_in_values: a blank line (not a comment) extends the current value.
            let has_comments = stripped != clean;
            let name = optname
                .as_deref()
                .filter(|n| !n.is_empty() && !has_comments && cur != Cur::None);
            if let Some(values) = name.and_then(|n| {
                options_mut(&mut defaults, &mut sections, &cur).and_then(|o| find_mut(o, n))
            }) {
                values.push(String::new());
            }
            continue;
        }
        let cur_indent = line.chars().position(|c| !is_space(c)).unwrap_or(0);
        // Continuation line?
        if cur != Cur::None
            && optname.as_deref().is_some_and(|n| !n.is_empty())
            && cur_indent > indent_level
        {
            let name = optname.clone().unwrap_or_default();
            if let Some(values) =
                options_mut(&mut defaults, &mut sections, &cur).and_then(|o| find_mut(o, &name))
            {
                values.push(clean.to_owned());
            }
            continue;
        }
        indent_level = cur_indent;
        // A section header: `\[(?P<header>.+)\]` matched at the start: up to the last `]`, non-empty.
        let header = clean
            .strip_prefix('[')
            .and_then(|rest| {
                rest.rfind(']')
                    .map(|end| rest.get(..end).unwrap_or_default())
            })
            .filter(|h| !h.is_empty());
        if let Some(h) = header {
            h.clone_into(&mut sectname);
            if let Some(&i) = section_index.get(h) {
                if added_sections.contains(h) {
                    return Err(crash(
                        "configparser.DuplicateSectionError",
                        format!(
                            "While reading from {} [line {lineno:2}]: section {} already exists",
                            repr_str(source),
                            repr_str(h)
                        ),
                    ));
                }
                cur = Cur::Section(i);
                added_sections.insert(h.to_owned());
            } else if h == "DEFAULT" {
                cur = Cur::Default;
            } else {
                let i = sections.len();
                sections.push((h.to_owned(), Vec::new()));
                section_index.insert(h.to_owned(), i);
                added_sections.insert(h.to_owned());
                cur = Cur::Section(i);
            }
            optname = None;
            continue;
        }
        if cur == Cur::None {
            return Err(ReadError::MissingSectionHeader);
        }
        // An option line: the option is everything before the first delimiter.
        let Some(p) = clean.find(['=', ':']) else {
            errors.push((lineno, line));
            continue;
        };
        let opt_raw = clean.get(..p).unwrap_or_default();
        if opt_raw.is_empty() {
            errors.push((lineno, line));
        }
        let name = rstrip(opt_raw).to_lowercase();
        let key = (sectname.clone(), name.clone());
        if added_options.contains(&key) {
            return Err(crash(
                "configparser.DuplicateOptionError",
                format!(
                    "While reading from {} [line {lineno:2}]: option {} in section {} already exists",
                    repr_str(source),
                    repr_str(&name),
                    repr_str(&sectname)
                ),
            ));
        }
        added_options.insert(key);
        let value = strip(clean.get(p.saturating_add(1)..).unwrap_or_default()).to_owned();
        if let Some(opts) = options_mut(&mut defaults, &mut sections, &cur) {
            match find_mut(opts, &name) {
                Some(values) => *values = vec![value],
                None => opts.push((name.clone(), vec![value])),
            }
        }
        optname = Some(name);
    }
    if !errors.is_empty() {
        let mut msg = format!("Source contains parsing errors: {}", repr_str(source));
        for (lineno, line) in errors {
            write!(msg, "\n\t[line {lineno:2}]: {}", repr_str(line)).unwrap_or_default();
        }
        return Err(crash("configparser.ParsingError", msg));
    }
    let join = |opts: RawOptions| -> Section {
        let mut s = Section::default();
        for (name, values) in opts {
            let joined = rstrip(&values.join("\n")).to_owned();
            s.index.insert(name.clone(), s.options.len());
            s.options.push((name, joined));
        }
        s
    };
    Ok(Parsed {
        defaults: join(defaults),
        sections: sections.into_iter().map(|(n, o)| (n, join(o))).collect(),
    })
}

fn options_mut<'a>(
    defaults: &'a mut RawOptions,
    sections: &'a mut [(String, RawOptions)],
    cur: &Cur,
) -> Option<&'a mut RawOptions> {
    match cur {
        Cur::None => None,
        Cur::Default => Some(defaults),
        Cur::Section(i) => sections.get_mut(*i).map(|(_, o)| o),
    }
}

fn find_mut<'a>(opts: &'a mut [(String, Vec<String>)], name: &str) -> Option<&'a mut Vec<String>> {
    opts.iter_mut().find(|(n, _)| n == name).map(|(_, v)| v)
}

/// `BasicInterpolation.before_get` → `_interpolate_some` (iterative; `%%`, `%(name)s`, depth 10).
fn interpolate(p: &Parsed, section: &str, option: &str, value: &str) -> Result<String, CheckError> {
    let syntax = |msg: String| CheckError::crash("configparser.InterpolationSyntaxError", msg);
    let mut out = String::new();
    // Stack of (rest of a value, depth).
    let mut stack: Vec<(String, usize)> = vec![(value.to_owned(), 1)];
    while let Some((mut rest, depth)) = stack.pop() {
        if depth > MAX_INTERPOLATION_DEPTH {
            return Err(CheckError::crash(
                "configparser.InterpolationDepthError",
                format!(
                    "Recursion limit exceeded in value substitution: option {} in section {} contains an \
                     interpolation key which cannot be substituted in {MAX_INTERPOLATION_DEPTH} steps. Raw value: {}",
                    repr_str(option),
                    repr_str(section),
                    repr_str(value)
                ),
            ));
        }
        while !rest.is_empty() {
            let Some(pct) = rest.find('%') else {
                out.push_str(&rest);
                rest.clear();
                break;
            };
            out.push_str(rest.get(..pct).unwrap_or_default());
            rest = rest.get(pct..).unwrap_or_default().to_owned();
            let next = rest.get(1..).and_then(|r| r.chars().next());
            match next {
                Some('%') => {
                    out.push('%');
                    rest = rest.get(2..).unwrap_or_default().to_owned();
                }
                Some('(') => {
                    // `%\(([^)]+)\)s` at the start.
                    let after = rest.get(2..).unwrap_or_default();
                    let close = after.find(')');
                    let name = close.and_then(|c| after.get(..c)).filter(|n| !n.is_empty());
                    let Some(name) = name.filter(|n| {
                        after
                            .get(n.len().saturating_add(1)..)
                            .is_some_and(|r| r.starts_with('s'))
                    }) else {
                        return Err(syntax(format!(
                            "bad interpolation variable reference {}",
                            repr_str(&rest)
                        )));
                    };
                    let var = name.to_lowercase();
                    let consumed = name.len().saturating_add(4);
                    let tail = rest.get(consumed..).unwrap_or_default().to_owned();
                    let Some(v) = p.lookup(section, &var) else {
                        return Err(CheckError::crash(
                            "configparser.InterpolationMissingOptionError",
                            format!(
                                "Bad value substitution: option {} in section {} contains an interpolation key {} \
                                 which is not a valid option name. Raw value: {}",
                                repr_str(option),
                                repr_str(section),
                                repr_str(&var),
                                repr_str(value)
                            ),
                        ));
                    };
                    if v.contains('%') {
                        // Recurse into `v`, then continue with `tail` at this depth.
                        stack.push((tail, depth));
                        stack.push((v.to_owned(), depth.saturating_add(1)));
                        break;
                    }
                    out.push_str(v);
                    rest = tail;
                }
                _ => {
                    return Err(syntax(format!(
                        "'%' must be followed by '%' or '(', found: {}",
                        repr_str(&rest)
                    )));
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        let text = "[default]\naws_access_key_id = AKIA\naws_secret_access_key = abc \n[other]\nAWS_SESSION_TOKEN: t\n";
        assert_eq!(
            aws_credentials(text).unwrap(),
            [
                ("aws_secret_access_key".to_owned(), "abc".to_owned()),
                ("aws_session_token".to_owned(), "t".to_owned())
            ]
        );
        assert_eq!(aws_credentials("no header\n").unwrap(), []);
        assert_eq!(
            aws_credentials("[a]\n[a]\n").unwrap_err(),
            "configparser.DuplicateSectionError: While reading from '<string>' [line  2]: section 'a' already exists"
        );
        assert_eq!(
            aws_credentials("[a]\naws_secret_access_key = 5%b\n").unwrap_err(),
            "configparser.InterpolationSyntaxError: '%' must be followed by '%' or '(', found: '%b'"
        );
    }

    #[test]
    fn default_section_and_interpolation() {
        let text = "[DEFAULT]\nbase = s3cr3t\n[p]\naws_secret_access_key = %(base)s-%%x\n[q]\n";
        assert_eq!(
            aws_credentials(text).unwrap(),
            [("aws_secret_access_key".to_owned(), "s3cr3t-%x".to_owned())]
        );
        let text = "[DEFAULT]\naws_secret_access_key = inherited\n[p]\n[q]\n";
        assert_eq!(aws_credentials(text).unwrap().len(), 2);
    }

    #[test]
    fn continuation_and_parsing_errors() {
        let text = "[a]\naws_session_token = one\n  two\n\n  three\n";
        assert_eq!(
            aws_credentials(text).unwrap(),
            [(
                "aws_session_token".to_owned(),
                "one\ntwo\n\nthree".to_owned()
            )]
        );
        assert_eq!(
            aws_credentials("[a]\nnot an option\n").unwrap_err(),
            "configparser.ParsingError: Source contains parsing errors: '<string>'\n\t[line  2]: 'not an option\\n'"
        );
    }
}
