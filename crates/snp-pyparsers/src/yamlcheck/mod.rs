//! `check-yaml`: a ruamel.yaml ≥ 0.19-compatible checker (09 §4.C, §9.5; `check_yaml.py`).
//!
//! A port of the pure-Python pipeline `YAML(typ='safe')` runs — `reader.py`, `scanner.py`, `parser.py`,
//! `composer.py`, the `VersionedResolver` and `SafeConstructor` — statement for statement, so acceptance, every
//! message text and every mark are ruamel 0.19.1's. The file is read the way the hook opens it (text mode,
//! strict UTF-8, universal newlines, lazily in chunks), and the constructor builds a model of the Python objects
//! (ints of any size, floats, dates, the shared NaN, tuples, …) so that duplicate keys follow Python's `dict`
//! membership and messages print `str()` of the objects.
//!
//! The hook's process is one `YAML` object for all files, and ruamel keeps state on it: a `%YAML 1.1` directive
//! leaks into the next file, and a failed construction leaves pending generators (and `deep_construct`) that run
//! during the next file. [`Checker`] is that process; [`check`] and [`check_ex`] run a fresh one per file.
//!
//! ruamel is recursive. CPython's recursion limit (1000 frames, `main` at depth 2) is reproduced by counting the
//! Python frames every ported function, property and constructor would push (`frame`/`touch`), including the
//! one-time costs of compiling the resolver's regular expressions and of the first `isinstance(x, Hashable)`
//! per type; past the limit the run crashes with `RecursionError` exactly where Python's would. The Rust side is
//! iterative where Python's recursion is unbounded in the input (composer, mapping keys, `repr`).
//!
//! Not reproduced (messages only, never acceptance): the warnings ruamel prints to stderr
//! (`ReusedAnchorWarning`, `MantissaNoDotYAML1_1Warning`), and the iteration order of a `set` printed inside a
//! duplicate-key message (CPython's is hash order, randomised per process for strings; snp prints insertion
//! order).
//!
//! Owner: WP-17.

mod composer;
mod construct;
mod int;
mod obj;
mod parser;
mod reader;
mod resolver;
mod scanner;
mod textio;

use std::rc::Rc;

use crate::CheckError;
use crate::json::PyMinor;
use crate::pystr;

/// `check_yaml.py` loader modes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Mode {
    /// Default: `YAML(typ='safe')`, one document.
    Safe,
    /// `--allow-multiple-documents` / `-m`.
    AllowMultipleDocuments,
    /// `--unsafe`: syntax check only.
    Unsafe,
}

/// `check_yaml.py`: `Err` carries ruamel.yaml's message (≥ 0.19 message classes).
///
/// One file in a fresh hook process under CPython 3.13, named `<file>` in the marks (ruamel's name for a
/// stream without one); `Err` is what the hook prints, or for the crash path the traceback's last line. The
/// hook itself uses [`Checker`] so that the marks carry the filename and state carries over between files.
pub fn check(text: &str, m: Mode) -> Result<(), String> {
    Checker::new(m, PyMinor::P313)
        .check_text(text, "<file>")
        .map_err(CheckError::into_message)
}

/// [`check`] on the file's bytes with its name and the emulated CPython minor (ARCHITECTURE §1.4.11 addition).
pub fn check_ex(bytes: &[u8], m: Mode, name: &str, v: PyMinor) -> Result<(), CheckError> {
    Checker::new(m, v).check_file(bytes, name)
}

/// One `check-yaml` process: the module-global `YAML(typ='safe')` and what it remembers between files
/// (ARCHITECTURE §1.4.11 addition).
#[derive(Debug)]
pub struct Checker {
    mode: Mode,
    st: State,
    crashed: Option<CheckError>,
}

impl Checker {
    /// A fresh process checking files in mode `m` under CPython `v`.
    pub fn new(m: Mode, v: PyMinor) -> Self {
        Self {
            mode: m,
            st: State {
                minor: v,
                ..State::default()
            },
            crashed: None,
        }
    }

    /// The hook's loop body for one file: `with open(filename, encoding='UTF-8') as f: load_fn(f)`.
    /// `Err(Reported)` is `str(exc)` of the `YAMLError` the hook prints (it then goes on with the next file);
    /// `Err(Crash)` is an uncaught exception: the process is gone and every later call returns the same crash.
    pub fn check_file(&mut self, bytes: &[u8], name: &str) -> Result<(), CheckError> {
        if let Some(c) = &self.crashed {
            return Err(c.clone());
        }
        let r = self.run(bytes, name);
        if let Err(e @ CheckError::Crash(_)) = &r {
            self.crashed = Some(e.clone());
        }
        r
    }

    /// [`Checker::check_file`] on already decoded text.
    pub fn check_text(&mut self, text: &str, name: &str) -> Result<(), CheckError> {
        self.check_file(text.as_bytes(), name)
    }

    /// Marks every one-time cost of the process as paid (compiled patterns, ABC caches, warning machinery), as
    /// in a long-lived interpreter; for the frame-model tests.
    #[doc(hidden)]
    pub fn warm(&mut self) {
        self.st.compiled = [true; 13];
        self.st.hashable_seen = [true; 13];
        self.st.default_tags_cached = [true; 3];
        self.st.warned = true;
    }

    /// The deepest Python frame (`main` = 2) the last call of [`Checker::check_file`] reached, as modelled.
    #[doc(hidden)]
    pub fn frame_peak(&self) -> u32 {
        self.st.peak
    }

    fn run(&mut self, bytes: &[u8], name: &str) -> Result<(), CheckError> {
        let outcome = Loader::new(&mut self.st, bytes, name).load(self.mode);
        match outcome {
            Ok(()) => Ok(()),
            Err(Fail::Crash(ty, text)) => Err(CheckError::crash(ty, text)),
            Err(Fail::Yaml(msg)) => {
                // `print(exc)`: a lone surrogate (from a `\ud800` escape) cannot be written to stdout.
                if let Some(err) = pystr::encode_error(&msg, 0) {
                    return Err(CheckError::crash("UnicodeEncodeError", err));
                }
                Err(CheckError::Reported(pystr::to_string_lossy(&msg)))
            }
        }
    }
}

/// CPython's recursion limit; `main` runs at depth 2 (`<module>` is 1).
pub(super) const LIMIT: u32 = 1000;

/// The depth of `main` in the console script.
const MAIN_DEPTH: u32 = 2;

/// How a step failed.
#[derive(Debug)]
pub(super) enum Fail {
    /// A `YAMLError`: `str(exc)` in generalized UTF-8 (it may hold lone surrogates).
    Yaml(Vec<u8>),
    /// Any other exception: class name and `str(exc)`.
    Crash(&'static str, String),
}

impl Fail {
    pub(super) fn crash(ty: &'static str, text: impl Into<String>) -> Self {
        Self::Crash(ty, text.into())
    }

    pub(super) fn recursion() -> Self {
        Self::Crash(
            "RecursionError",
            "maximum recursion depth exceeded".to_owned(),
        )
    }
}

pub(super) type R<T> = Result<T, Fail>;

/// A `FileMark`: 0-based line and column, code-point index; printed 1-based.
#[derive(Debug, Clone)]
pub(super) struct Mark {
    pub(super) name: Rc<str>,
    pub(super) line: usize,
    pub(super) column: usize,
}

impl Mark {
    /// `StreamMark.__str__`.
    fn render(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(
            format!(
                "  in \"{}\", line {}, column {}",
                self.name,
                self.line.saturating_add(1),
                self.column.saturating_add(1)
            )
            .as_bytes(),
        );
    }

    fn same_place(&self, o: &Self) -> bool {
        self.name == o.name && self.line == o.line && self.column == o.column
    }
}

/// `MarkedYAMLError.__str__` with an already dedented note.
pub(super) fn marked(
    context: Option<&str>,
    context_mark: Option<&Mark>,
    problem: Option<&[u8]>,
    problem_mark: Option<&Mark>,
    note: Option<&str>,
) -> Fail {
    let mut lines: Vec<Vec<u8>> = Vec::new();
    if let Some(c) = context {
        lines.push(c.as_bytes().to_vec());
    }
    if let Some(cm) = context_mark {
        let distinct = match (problem, problem_mark) {
            (Some(_), Some(pm)) => !cm.same_place(pm),
            _ => true,
        };
        if distinct {
            let mut l = Vec::new();
            cm.render(&mut l);
            lines.push(l);
        }
    }
    if let Some(p) = problem {
        lines.push(p.to_vec());
    }
    if let Some(pm) = problem_mark {
        let mut l = Vec::new();
        pm.render(&mut l);
        lines.push(l);
    }
    if let Some(n) = note
        && !n.is_empty()
    {
        lines.push(n.as_bytes().to_vec());
    }
    Fail::Yaml(lines.join(&b'\n'))
}

/// A YAML version (`%YAML major.minor`), each part a Python `int` kept as its normalised decimal digits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Ver(pub(super) String, pub(super) String);

impl Ver {
    pub(super) fn v12() -> Self {
        Self("1".into(), "2".into())
    }

    pub(super) fn is(&self, major: &str, minor: &str) -> bool {
        self.0 == major && self.1 == minor
    }

    pub(super) fn is11(&self) -> bool {
        self.is("1", "1")
    }

    /// `self > (1, 1)` as tuples of ints.
    pub(super) fn gt11(&self) -> bool {
        let num = |s: &str| (s.len(), s.to_owned());
        let (a, b) = (num(&self.0), num(&self.1));
        let one = num("1");
        a > one || a == one && b > one
    }

    /// `repr` of the tuple: `(1, 3)`.
    pub(super) fn repr(&self) -> String {
        format!("({}, {})", self.0, self.1)
    }
}

/// The regular expressions ruamel compiles lazily (`RegExp = partial(LazyEval, re.compile)`), with the frame
/// depth (relative to the calling function) that the first use reaches while `re.compile` parses the pattern;
/// later uses cost `LazyEval.__getattribute__` plus the `lazy_self` lambda (2). Measured on CPython 3.12.3 and
/// 3.13 (identical).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Rx {
    Bool12,
    Bool11,
    Float12,
    Float11,
    Int12,
    Int11,
    Merge,
    Null,
    Timestamp,
    Value,
    Yaml,
    NonPrintable,
    TimestampCons,
}

impl Rx {
    const fn index(self) -> usize {
        match self {
            Self::Bool12 => 0,
            Self::Bool11 => 1,
            Self::Float12 => 2,
            Self::Float11 => 3,
            Self::Int12 => 4,
            Self::Int11 => 5,
            Self::Merge => 6,
            Self::Null => 7,
            Self::Timestamp => 8,
            Self::Value => 9,
            Self::Yaml => 10,
            Self::NonPrintable => 11,
            Self::TimestampCons => 12,
        }
    }
}

const RX_FIRST_DEPTH: [u32; 13] = [12, 12, 14, 14, 12, 14, 12, 12, 18, 12, 12, 10, 20];

/// The object kinds whose first `isinstance(obj, Hashable)` runs `ABCMeta.__subclasscheck__` (4 frames).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum PyType {
    NoneType,
    Bool,
    Int,
    Float,
    Str,
    Bytes,
    Date,
    DateTime,
    List,
    Tuple,
    Dict,
    Set,
    ODict,
}

impl PyType {
    const fn index(self) -> usize {
        match self {
            Self::NoneType => 0,
            Self::Bool => 1,
            Self::Int => 2,
            Self::Float => 3,
            Self::Str => 4,
            Self::Bytes => 5,
            Self::Date => 6,
            Self::DateTime => 7,
            Self::List => 8,
            Self::Tuple => 9,
            Self::Dict => 10,
            Self::Set => 11,
            Self::ODict => 12,
        }
    }
}

/// What the module-global `YAML` object (and the interpreter) remember between files.
#[derive(Debug)]
pub(super) struct State {
    minor: PyMinor,
    /// `YAML.version`, set by every explicit document's directives.
    version: Option<Ver>,
    /// The resolver (recreated whenever `YAML.version` changes) and the versions whose resolver tables it has
    /// built (`add_version_implicit_resolver` runs on first use).
    resolver_loader_version: Option<ResolverVersion>,
    resolver_built: Vec<Ver>,
    compiled: [bool; 13],
    hashable_seen: [bool; 13],
    /// Whether `trval` of the three default `Tag` objects of `BaseResolver` was computed.
    default_tags_cached: [bool; 3],
    /// The frame peak of the last file.
    peak: u32,
    /// Whether a warning was displayed already (the first display imports what `linecache` needs).
    warned: bool,
    /// Whether the YAML object's components exist (the first file creates them).
    created: bool,
    cons: construct::ConsState,
}

impl Default for State {
    fn default() -> Self {
        Self {
            minor: PyMinor::P313,
            version: None,
            resolver_loader_version: None,
            resolver_built: Vec::new(),
            compiled: [false; 13],
            hashable_seen: [false; 13],
            peak: 0,
            default_tags_cached: [false; 3],
            warned: false,
            created: false,
            cons: construct::ConsState::default(),
        }
    }
}

/// The `_loader_version` the current resolver was created with (`YAML.version` at that time).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolverVersion(Option<Ver>);

/// One `load`/`load_all`/`parse` call: the reader, scanner, parser, composer and constructor of ruamel's
/// loader with their state for this file, and the Python frame depth.
pub(super) struct Loader<'a, 's> {
    st: &'s mut State,
    name: Rc<str>,
    depth: u32,
    /// The deepest frame reached (for the frame-model tests).
    peak: u32,
    io: textio::TextIo<'a>,
    r: reader::ReaderState,
    sc: scanner::ScannerState,
    p: parser::ParserState,
    anchors: std::collections::HashMap<String, construct::NodeId>,
}

impl<'a, 's> Loader<'a, 's> {
    fn new(st: &'s mut State, bytes: &'a [u8], name: &str) -> Self {
        let mut l = Self {
            st,
            name: Rc::from(name),
            depth: MAIN_DEPTH,
            peak: MAIN_DEPTH,
            io: textio::TextIo::new(bytes),
            r: reader::ReaderState::default(),
            sc: scanner::ScannerState::default(),
            p: parser::ParserState::default(),
            anchors: std::collections::HashMap::new(),
        };
        l.scanner_start();
        l
    }

    /// Runs `f` as one Python call: one frame deeper, `RecursionError` past the limit.
    pub(super) fn frame<T>(&mut self, f: impl FnOnce(&mut Self) -> R<T>) -> R<T> {
        let saved = self.depth;
        self.depth = saved.saturating_add(1);
        self.peak = self.peak.max(self.depth);
        if self.depth > LIMIT {
            return Err(Fail::recursion());
        }
        let r = f(self);
        self.depth = saved;
        r
    }

    /// Pushes a Python frame by hand (for the explicit stacks); returns the depth to restore.
    pub(super) fn push_frame(&mut self) -> R<u32> {
        let saved = self.depth;
        self.depth = saved.saturating_add(1);
        self.peak = self.peak.max(self.depth);
        if self.depth > LIMIT {
            return Err(Fail::recursion());
        }
        Ok(saved)
    }

    /// A call chain `k` frames deep that returns without deeper calls (properties, `__init__` chains).
    pub(super) fn touch(&mut self, k: u32) -> R<()> {
        self.peak = self.peak.max(self.depth.saturating_add(k));
        if self.depth.saturating_add(k) > LIMIT {
            Err(Fail::recursion())
        } else {
            Ok(())
        }
    }

    /// `regexp.match(…)` / `.search(…)` through `LazyEval` (compiling the pattern on first use).
    pub(super) fn rx_call(&mut self, rx: Rx) -> R<()> {
        let i = rx.index();
        let first = !self.st.compiled.get(i).copied().unwrap_or(true);
        let k = if first {
            RX_FIRST_DEPTH.get(i).copied().unwrap_or(2)
        } else {
            2
        };
        self.touch(k)?;
        if let Some(c) = self.st.compiled.get_mut(i) {
            *c = true;
        }
        Ok(())
    }

    /// `isinstance(obj, Hashable)`.
    pub(super) fn hashable_check(&mut self, t: PyType) -> R<()> {
        let i = t.index();
        let first = !self.st.hashable_seen.get(i).copied().unwrap_or(true);
        self.touch(if first { 4 } else { 1 })?;
        if let Some(c) = self.st.hashable_seen.get_mut(i) {
            *c = true;
        }
        Ok(())
    }

    /// `VersionedResolver.processing_version`: the scanner's `%YAML` version, else `YAML.version`, else 1.2.
    pub(super) fn processing_version(&self) -> Ver {
        self.sc
            .yaml_version
            .clone()
            .or_else(|| self.st.version.clone())
            .unwrap_or_else(Ver::v12)
    }

    /// `self.loader.resolver` (the `YAML.resolver` property, which recreates the resolver when `YAML.version`
    /// changed: `VersionedResolver.__init__` → `BaseResolver.__init__`, `get_loader_version`) followed by its
    /// `processing_version` property: the frames below the caller.
    pub(super) fn resolver_pv(&mut self) -> R<Ver> {
        self.resolver_prop()?;
        self.touch(1)?;
        Ok(self.processing_version())
    }

    /// The `YAML.resolver` property one frame below the caller (it reads the `YAML.version` property; a change
    /// of version recreates the resolver).
    pub(super) fn resolver_prop(&mut self) -> R<()> {
        let want = Some(ResolverVersion(self.st.version.clone()));
        if self.st.resolver_loader_version == want {
            self.touch(2)?; // YAML.resolver -> the YAML.version property
        } else {
            self.touch(3)?;
            self.st.resolver_loader_version = want;
            self.st.resolver_built.clear();
        }
        Ok(())
    }

    /// `YAML.load` / `_load_all` / `_parse_unsafe` on this file (`check_yaml.LOAD_FNS`).
    fn load(mut self, mode: Mode) -> R<()> {
        // A file starts with whatever the previous one left: arenas of a finished run are dropped.
        self.st.cons.gc();
        let r = match mode {
            Mode::Safe => self.frame(Self::load_single),
            Mode::AllowMultipleDocuments => self.frame(|s| {
                // _load_all -> _exhaust -> the load_all generator
                s.frame(|s| s.frame(Self::load_all))
            }),
            Mode::Unsafe => self.frame(|s| s.frame(|s| s.frame(Self::parse_all))),
        };
        self.st.created = true;
        self.st.peak = self.peak;
        r
    }

    /// The common prologue of `load`/`load_all`/`parse`: `DocInfo(requested_version=version(self.version))`
    /// and `get_constructor_parser(stream)` / `self.parser` with `self.reader.stream = stream`.
    fn prologue(&mut self, with_constructor: bool) -> R<()> {
        self.touch(1)?; // version()
        self.touch(1)?; // DocInfo.__init__
        let created = self.st.created;
        self.frame(|s| {
            if with_constructor {
                // get_constructor_parser
                if !created {
                    s.touch(3)?; // YAML.reader -> Reader.__init__ -> reset_reader
                }
                s.touch(1)?;
            }
            s.reader_open()?;
            if with_constructor {
                if !created {
                    s.touch(3)?; // YAML.constructor -> SafeConstructor.__init__ -> version_tnf
                    s.touch(3)?; // YAML.parser -> Parser.__init__ -> reset_parser
                }
                s.touch(1)?;
            }
            Ok(())
        })
    }

    /// The epilogue's `reset_reader`/`reset_scanner` need no frames worth counting (shallow).
    fn load_single(&mut self) -> R<()> {
        self.prologue(true)?;
        // constructor.get_single_data()
        self.frame(|s| {
            s.touch(2)?; // the composer property
            let node = s.frame(Self::get_single_node)?;
            if let Some(node) = node {
                s.frame(|s| s.construct_document(node))?;
            }
            Ok(())
        })
    }

    fn load_all(&mut self) -> R<()> {
        self.prologue(true)?;
        loop {
            // constructor.check_data() -> composer.check_node()
            let more = self.frame(|s| {
                s.touch(2)?;
                s.frame(Self::check_node)
            })?;
            if !more {
                return Ok(());
            }
            // constructor.get_data()
            self.frame(|s| {
                s.touch(2)?;
                if s.frame(Self::check_node)? {
                    let node = s.frame(Self::get_node)?;
                    if let Some(node) = node {
                        s.frame(|s| s.construct_document(node))?;
                    }
                }
                Ok(())
            })?;
            self.touch(1)?; // version() / DocInfo for the next document
        }
    }

    fn parse_all(&mut self) -> R<()> {
        self.prologue(true)?;
        while self.check_event_any()? {
            self.get_event()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
