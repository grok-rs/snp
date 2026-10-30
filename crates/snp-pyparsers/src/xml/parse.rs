//! expat's processors (`xmlparse.c` 2.6): `doProlog` with the internal subset and internal parameter entities,
//! `doContent` with internal general entities, `doCdataSection`, `epilogProcessor`, `storeAtts` (duplicate
//! attributes, attribute-value normalization), `storeEntityValue`, `processXmlDecl` and the unknown-encoding
//! handler as pyexpat installs it, plus the input-amplification accounting. Entity expansion runs on explicit
//! stacks. The document is parsed as one final buffer (pyexpat's 64 KiB chunks give the same result).
//!
//! Owner: WP-17.

use std::collections::HashMap;
use std::rc::Rc;

use super::enc::{self, Builtin, Enc};
use super::role::{PrologState, Role};
use super::tok::{self, Tk};
use super::{Codec, Fail};
use crate::json::PyMinor;

/// `enum XML_Error` (the codes that can occur here) with `XML_ErrorString`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum XmlError {
    Syntax,
    NoElements,
    InvalidToken,
    UnclosedToken,
    PartialChar,
    TagMismatch,
    DuplicateAttribute,
    JunkAfterDocElement,
    ParamEntityRef,
    UndefinedEntity,
    RecursiveEntityRef,
    AsyncEntity,
    BadCharRef,
    BinaryEntityRef,
    AttributeExternalEntityRef,
    MisplacedXmlPi,
    UnknownEncoding,
    IncorrectEncoding,
    UnclosedCdataSection,
    UnexpectedState,
    EntityDeclaredInPe,
    IncompletePe,
    XmlDecl,
    Publicid,
    AmplificationLimitBreach,
}

impl XmlError {
    pub(super) const fn text(self) -> &'static str {
        match self {
            Self::Syntax => "syntax error",
            Self::NoElements => "no element found",
            Self::InvalidToken => "not well-formed (invalid token)",
            Self::UnclosedToken => "unclosed token",
            Self::PartialChar => "partial character",
            Self::TagMismatch => "mismatched tag",
            Self::DuplicateAttribute => "duplicate attribute",
            Self::JunkAfterDocElement => "junk after document element",
            Self::ParamEntityRef => "illegal parameter entity reference",
            Self::UndefinedEntity => "undefined entity",
            Self::RecursiveEntityRef => "recursive entity reference",
            Self::AsyncEntity => "asynchronous entity",
            Self::BadCharRef => "reference to invalid character number",
            Self::BinaryEntityRef => "reference to binary entity",
            Self::AttributeExternalEntityRef => "reference to external entity in attribute",
            Self::MisplacedXmlPi => "XML or text declaration not at start of entity",
            Self::UnknownEncoding => "unknown encoding",
            Self::IncorrectEncoding => "encoding specified in XML declaration is incorrect",
            Self::UnclosedCdataSection => "unclosed CDATA section",
            Self::UnexpectedState => {
                "unexpected parser state.saturating_sub(please) send a bug report"
            }
            Self::EntityDeclaredInPe => "entity declared in parameter entity",
            Self::IncompletePe => "incomplete markup in parameter entity",
            Self::XmlDecl => "XML declaration not well-formed",
            Self::Publicid => "illegal character(s) in public id",
            Self::AmplificationLimitBreach => {
                "limit on input amplification factor (from DTD and entities) breached"
            }
        }
    }
}

/// A parse failure: an expat error (reported) or a Python exception from the unknown-encoding handler.
#[derive(Debug)]
pub(super) enum Stop {
    Xml(XmlError),
    Py(Fail),
}

type Res<T> = Result<T, Stop>;

const fn err<T>(e: XmlError) -> Res<T> {
    Err(Stop::Xml(e))
}

#[derive(Debug, Default)]
#[expect(clippy::struct_excessive_bools, reason = "the flags of expat's ENTITY")]
struct Entity {
    /// Replacement text (internal UTF-8) of an internal entity.
    text: Option<Rc<[u8]>>,
    system: bool,
    notation: bool,
    is_internal: bool,
    open: bool,
}

/// Where a token stream comes from: the document, or an internal entity's replacement text.
#[derive(Clone)]
enum Src {
    Doc,
    Text(Rc<[u8]>),
}

/// expat's `XML_ACCOUNT_*`.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Account {
    Direct,
    Expansion,
    None,
}

/// `ENTITY_*` declarations being processed (`m_declEntity`).
#[derive(Clone)]
struct DeclEntity {
    param: bool,
    name: Vec<u8>,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "the flags of expat's parser and DTD"
)]
pub(super) struct Parser<'a> {
    doc: &'a [u8],
    pub(super) enc: Enc,
    minor: PyMinor,
    prolog: PrologState,
    /// `m_eventPtr` in the document.
    pub(super) event: usize,
    general: HashMap<Vec<u8>, Entity>,
    param: HashMap<Vec<u8>, Entity>,
    has_param_entity_refs: bool,
    standalone: bool,
    keep_processing: bool,
    param_entity_parsing: bool,
    doctype_sysid: bool,
    decl_entity: Option<DeclEntity>,
    /// Open internal entities (`m_openInternalEntities` is non-null).
    open_internal: usize,
    tags: Vec<(Vec<u8>, bool)>,
    /// `m_groupConnector`: the separator seen at each content-model group level (0, `,` or `|`).
    group_connector: Vec<u8>,
    direct: u64,
    indirect: u64,
}

/// What the prolog loop does at the end of a parameter entity's text.
struct PeFrame {
    src: Src,
    pos: usize,
    end: usize,
    name: Vec<u8>,
}

impl<'a> Parser<'a> {
    pub(super) fn new(doc: &'a [u8], minor: PyMinor) -> Self {
        Self {
            doc,
            enc: Enc::Utf8,
            minor,
            prolog: PrologState::default(),
            event: 0,
            general: HashMap::new(),
            param: HashMap::new(),
            has_param_entity_refs: false,
            standalone: false,
            keep_processing: true,
            param_entity_parsing: true,
            doctype_sysid: false,
            decl_entity: None,
            open_internal: 0,
            tags: Vec::new(),
            group_connector: Vec::new(),
            direct: 0,
            indirect: 0,
        }
    }

    fn buf<'b>(&'b self, src: &'b Src) -> &'b [u8] {
        match src {
            Src::Doc => self.doc,
            Src::Text(t) => t,
        }
    }

    fn enc_of(&self, src: &Src) -> Enc {
        match src {
            Src::Doc => self.enc.clone(),
            Src::Text(_) => Enc::Internal,
        }
    }

    /// `accountingDiffTolerated`.
    fn account(&mut self, t: i32, before: usize, after: usize, acc: Account) -> Res<()> {
        if matches!(
            t,
            tok::INVALID | tok::PARTIAL | tok::PARTIAL_CHAR | tok::NONE
        ) || acc == Account::None
        {
            return Ok(());
        }
        let n = u64::try_from(after.saturating_sub(before)).unwrap_or(0);
        if acc == Account::Direct {
            self.direct = self.direct.saturating_add(n);
        } else {
            self.indirect = self.indirect.saturating_add(n);
        }
        let total = self.direct.saturating_add(self.indirect);
        if total < 8 * 1024 * 1024 {
            return Ok(());
        }
        #[expect(
            clippy::cast_precision_loss,
            clippy::as_conversions,
            reason = "expat computes the factor in `float` from the two counts"
        )]
        let factor = if self.direct == 0 {
            1.0f32
        } else {
            total as f32 / self.direct as f32
        };
        if factor <= 100.0 {
            Ok(())
        } else {
            err(XmlError::AmplificationLimitBreach)
        }
    }

    // ------------------------------------------------------------------------------------------------------
    // initial encoding detection (`initScan`) and the prolog
    // ------------------------------------------------------------------------------------------------------

    /// `prologInitProcessor` + `prologProcessor`: detects the encoding from the first bytes and runs `doProlog`.
    pub(super) fn run(&mut self) -> Res<()> {
        let d = self.doc;
        let mut start = 0usize;
        let first_tok = if d.is_empty() {
            None
        } else if d.len() == 1 {
            matches!(d.first(), Some(0xFE | 0xFF | 0xEF | 0x00 | 0x3C))
                .then_some((tok::PARTIAL, None))
        } else {
            let b0 = d.first().copied().unwrap_or(0);
            let b1 = d.get(1).copied().unwrap_or(0);
            match (u16::from(b0) << 8) | u16::from(b1) {
                0xFEFF => {
                    self.enc = Enc::Big2;
                    Some((tok::BOM, Some(2)))
                }
                0x3C00 => {
                    self.enc = Enc::Little2;
                    None
                }
                0xFFFE => {
                    self.enc = Enc::Little2;
                    Some((tok::BOM, Some(2)))
                }
                0xEFBB => {
                    if d.len() == 2 {
                        Some((tok::PARTIAL, None))
                    } else if d.get(2) == Some(&0xBF) {
                        self.enc = Enc::Utf8;
                        Some((tok::BOM, Some(3)))
                    } else {
                        None
                    }
                }
                _ => {
                    if b0 == 0 {
                        self.enc = Enc::Big2;
                    } else if b1 == 0 {
                        self.enc = Enc::Little2;
                    }
                    None
                }
            }
        };
        let (t, next) = if let Some(t) = first_tok {
            t
        } else {
            let tk = Tk {
                enc: &self.enc,
                buf: d,
            };
            tk.prolog_tok(start, d.len())
        };
        if t == tok::BOM {
            // the BOM token goes through doProlog like any other
            let n = next.unwrap_or(0);
            self.do_prolog_tokens(t, start, n)?;
            start = n;
            let tk = Tk {
                enc: &self.enc,
                buf: d,
            };
            let (t2, n2) = tk.prolog_tok(start, d.len());
            return self.do_prolog(t2, start, n2);
        }
        self.do_prolog(t, start, next)
    }

    /// Handles the BOM token's role (it has none) and the accounting.
    fn do_prolog_tokens(&mut self, t: i32, s: usize, next: usize) -> Res<()> {
        self.account(t, s, next, Account::Direct)?;
        let tk = Tk {
            enc: &self.enc,
            buf: self.doc,
        };
        self.prolog.token_role(t, s, next, &tk);
        Ok(())
    }

    /// `doProlog` over the document from `s` (first token already scanned), with internal parameter entities
    /// pushed on a stack.
    fn do_prolog(&mut self, first: i32, s0: usize, next0: Option<usize>) -> Res<()> {
        let mut stack: Vec<PeFrame> = Vec::new();
        let mut src = Src::Doc;
        let mut s = s0;
        let mut t = first;
        let mut next = next0.unwrap_or(s0);
        let mut end = self.doc.len();
        loop {
            let in_doc = matches!(src, Src::Doc);
            if in_doc {
                self.event = s;
            }
            if t <= 0 {
                match t {
                    tok::INVALID => {
                        if in_doc {
                            self.event = next;
                        }
                        return err(XmlError::InvalidToken);
                    }
                    tok::PARTIAL => return err(XmlError::UnclosedToken),
                    tok::PARTIAL_CHAR => return err(XmlError::PartialChar),
                    t2 if t2 == -tok::PROLOG_S => t = tok::PROLOG_S,
                    tok::NONE => {
                        if !in_doc {
                            // the end of an internal parameter entity referenced between declarations
                            let tk_enc = Enc::Internal;
                            let buf = self.buf(&src).to_vec();
                            let tk = Tk {
                                enc: &tk_enc,
                                buf: &buf,
                            };
                            if self.prolog.token_role(tok::NONE, end, end, &tk) == Role::Error {
                                return err(XmlError::IncompletePe);
                            }
                            let Some(f) = stack.pop() else {
                                return err(XmlError::UnexpectedState);
                            };
                            if let Some(e) = self.param.get_mut(&f.name) {
                                e.open = false;
                            }
                            self.open_internal = self.open_internal.saturating_sub(1);
                            src = f.src;
                            s = f.pos;
                            end = f.end;
                            let enc = self.enc_of(&src);
                            let buf = self.buf(&src);
                            let (t2, n2) = Tk { enc: &enc, buf }.prolog_tok(s, end);
                            t = t2;
                            next = n2.unwrap_or(s);
                            continue;
                        }
                        return err(XmlError::NoElements);
                    }
                    _ => {
                        t = t.saturating_neg();
                        next = end;
                    }
                }
            }
            let acc = if in_doc {
                Account::Direct
            } else {
                Account::Expansion
            };
            self.account(t, s, next, acc)?;
            let enc = self.enc_of(&src);
            let buf: Rc<[u8]> = match &src {
                Src::Doc => Rc::from(&b""[..]),
                Src::Text(b) => Rc::clone(b),
            };
            let bufs: &[u8] = if in_doc { self.doc } else { &buf };
            let role = {
                let tk = Tk {
                    enc: &enc,
                    buf: bufs,
                };
                self.prolog.token_role(t, s, next, &tk)
            };
            match role {
                Role::XmlDecl => {
                    self.process_xml_decl(s, next)?;
                }
                Role::GroupOpen => {
                    let level = usize::try_from(self.prolog.level()).unwrap_or(0);
                    if self.group_connector.len() <= level {
                        self.group_connector.resize(level.saturating_add(1), 0);
                    }
                    if let Some(c) = self.group_connector.get_mut(level) {
                        *c = 0;
                    }
                }
                Role::GroupSequence | Role::GroupChoice => {
                    let level = usize::try_from(self.prolog.level()).unwrap_or(0);
                    let (bad, mine) = if role == Role::GroupSequence {
                        (b'|', b',')
                    } else {
                        (b',', b'|')
                    };
                    if self.group_connector.get(level).copied() == Some(bad) {
                        return err(XmlError::Syntax);
                    }
                    if self.group_connector.len() <= level {
                        self.group_connector.resize(level.saturating_add(1), 0);
                    }
                    if let Some(c) = self.group_connector.get_mut(level) {
                        *c = mine;
                    }
                }
                Role::NotationPublicId => {
                    let tk = Tk {
                        enc: &enc,
                        buf: bufs,
                    };
                    if let Err(bad) = tk.is_public_id(s, next) {
                        if in_doc {
                            self.event = bad;
                        }
                        return err(XmlError::Publicid);
                    }
                }
                Role::DoctypePublicId | Role::EntityPublicId => {
                    if role == Role::DoctypePublicId {
                        self.decl_entity = Some(DeclEntity {
                            param: true,
                            name: b"#".to_vec(),
                        });
                        self.has_param_entity_refs = true;
                    }
                    let tk = Tk {
                        enc: &enc,
                        buf: bufs,
                    };
                    if let Err(bad) = tk.is_public_id(s, next) {
                        if in_doc {
                            self.event = bad;
                        }
                        return err(XmlError::Publicid);
                    }
                }
                Role::DoctypeSystemId => {
                    self.doctype_sysid = true;
                    if self.decl_entity.is_none() {
                        self.decl_entity = Some(DeclEntity {
                            param: true,
                            name: b"#".to_vec(),
                        });
                    }
                    self.set_decl_system();
                }
                Role::EntitySystemId => self.set_decl_system(),
                Role::EntityNotationName => {
                    if self.keep_processing
                        && let Some(d) = self.decl_entity.clone()
                        && let Some(e) = self.entity_mut(&d)
                    {
                        e.notation = true;
                    }
                }
                Role::DoctypeClose => {
                    if !in_doc {
                        return err(XmlError::InvalidToken);
                    }
                    if self.doctype_sysid {
                        // the external subset: pyexpat's handler accepts it without reading it
                        self.has_param_entity_refs = true;
                    }
                }
                Role::InstanceStart => {
                    return self.do_content(s);
                }
                Role::GeneralEntityName | Role::ParamEntityName => {
                    let param = role == Role::ParamEntityName;
                    let tk = Tk {
                        enc: &enc,
                        buf: bufs,
                    };
                    if !param && tk.predefined_entity_name(s, next).is_some() {
                        self.decl_entity = None;
                    } else if self.keep_processing {
                        let name = enc.to_utf8(bufs, s, next);
                        let table = if param {
                            &mut self.param
                        } else {
                            &mut self.general
                        };
                        if table.contains_key(&name) {
                            self.decl_entity = None;
                        } else {
                            table.insert(
                                name.clone(),
                                Entity {
                                    is_internal: self.open_internal == 0,
                                    ..Entity::default()
                                },
                            );
                            self.decl_entity = Some(DeclEntity { param, name });
                        }
                    } else {
                        self.decl_entity = None;
                    }
                }
                Role::EntityValue => {
                    if self.keep_processing {
                        let m = enc.minbpc();
                        let value = self.store_entity_value(
                            &src,
                            s.saturating_add(m),
                            next.saturating_sub(m),
                        )?;
                        if let Some(d) = self.decl_entity.clone()
                            && let Some(e) = self.entity_mut(&d)
                        {
                            e.text = Some(Rc::from(value));
                        }
                    }
                }
                Role::DefaultAttributeValue | Role::FixedAttributeValue => {
                    if self.keep_processing {
                        let m = enc.minbpc();
                        self.append_attribute_value(
                            &src,
                            s.saturating_add(m),
                            next.saturating_sub(m),
                            true,
                            Account::None,
                        )?;
                    }
                }
                Role::Error => {
                    return match t {
                        tok::PARAM_ENTITY_REF => err(XmlError::ParamEntityRef),
                        tok::XML_DECL => err(XmlError::MisplacedXmlPi),
                        _ => err(XmlError::Syntax),
                    };
                }
                Role::ParamEntityRef => {
                    self.has_param_entity_refs = true;
                    if self.param_entity_parsing {
                        let m = enc.minbpc();
                        let name = enc.to_utf8(bufs, s.saturating_add(m), next.saturating_sub(m));
                        let exists = self.param.contains_key(&name);
                        let check = if self.standalone {
                            self.open_internal == 0
                        } else {
                            !self.has_param_entity_refs
                        };
                        if check {
                            match self.param.get(&name) {
                                None => return err(XmlError::UndefinedEntity),
                                Some(e) if !e.is_internal => {
                                    return err(XmlError::EntityDeclaredInPe);
                                }
                                _ => {}
                            }
                        } else if !exists {
                            self.keep_processing = self.standalone;
                            s = next;
                            let buf2 = self.buf(&src);
                            let (t2, n2) = Tk {
                                enc: &enc,
                                buf: buf2,
                            }
                            .prolog_tok(s, end);
                            t = t2;
                            next = n2.unwrap_or(s);
                            continue;
                        }
                        let (open, text) = match self.param.get(&name) {
                            Some(e) => (e.open, e.text.clone()),
                            None => (false, None),
                        };
                        if open {
                            return err(XmlError::RecursiveEntityRef);
                        }
                        if let Some(text) = text {
                            if let Some(e) = self.param.get_mut(&name) {
                                e.open = true;
                            }
                            self.open_internal = self.open_internal.saturating_add(1);
                            stack.push(PeFrame {
                                src: src.clone(),
                                pos: next,
                                end,
                                name,
                            });
                            src = Src::Text(Rc::clone(&text));
                            s = 0;
                            end = text.len();
                            let (t2, n2) = Tk {
                                enc: &Enc::Internal,
                                buf: &text,
                            }
                            .prolog_tok(0, end);
                            t = t2;
                            next = n2.unwrap_or(0);
                            continue;
                        }
                    }
                    // without parameter-entity parsing, or an external parameter entity (the handler returns
                    // without reading it)
                    self.keep_processing = self.standalone;
                }
                _ => {}
            }
            s = next;
            let enc = self.enc_of(&src);
            let buf2 = self.buf(&src);
            let (t2, n2) = Tk {
                enc: &enc,
                buf: buf2,
            }
            .prolog_tok(s, end);
            t = t2;
            next = n2.unwrap_or(s);
        }
    }

    fn entity_mut(&mut self, d: &DeclEntity) -> Option<&mut Entity> {
        if d.param {
            self.param.get_mut(&d.name)
        } else {
            self.general.get_mut(&d.name)
        }
    }

    fn set_decl_system(&mut self) {
        if self.keep_processing
            && let Some(d) = self.decl_entity.clone()
            && let Some(e) = self.entity_mut(&d)
        {
            e.system = true;
        }
    }

    /// `storeEntityValue` (`s..end` inside the quotes): the replacement text in internal UTF-8.
    fn store_entity_value(&mut self, src: &Src, s0: usize, end0: usize) -> Res<Vec<u8>> {
        struct Fr {
            src: Src,
            pos: usize,
            end: usize,
            name: Option<Vec<u8>>,
        }
        let mut out = Vec::new();
        let mut stack = vec![Fr {
            src: src.clone(),
            pos: s0,
            end: end0,
            name: None,
        }];
        loop {
            let depth = stack.len();
            let Some(fr) = stack.last_mut() else {
                break;
            };
            let in_doc = matches!(fr.src, Src::Doc);
            let enc = self.enc_of(&fr.src);
            let buf: &[u8] = match &fr.src {
                Src::Doc => self.doc,
                Src::Text(t) => t,
            };
            let p = fr.pos;
            let tk = Tk { enc: &enc, buf };
            let (t, nx) = tk.entity_value_tok(p, fr.end);
            let next = nx.unwrap_or(p);
            // (the entity value of a declaration is accounted for by the literal's token)
            let acc = if in_doc && depth == 1 {
                Account::None
            } else {
                Account::Expansion
            };
            let src_now = fr.src.clone();
            let buf_owned: Option<Rc<[u8]>> = match &src_now {
                Src::Doc => None,
                Src::Text(b) => Some(Rc::clone(b)),
            };
            self.account(t, p, next, acc)?;
            let buf: &[u8] = buf_owned.as_deref().unwrap_or(self.doc);
            let tk = Tk { enc: &enc, buf };
            match t {
                tok::PARAM_ENTITY_REF => {
                    if in_doc {
                        // In the internal subset, PE references are not allowed in markup declarations.
                        self.event = p;
                        return err(XmlError::ParamEntityRef);
                    }
                    let m = enc.minbpc();
                    let name = enc.to_utf8(buf, p.saturating_add(m), next.saturating_sub(m));
                    let (exists, open, system, text) = match self.param.get(&name) {
                        Some(e) => (true, e.open, e.system, e.text.clone()),
                        None => (false, false, false, None),
                    };
                    if !exists {
                        self.keep_processing = self.standalone;
                        return Ok(out);
                    }
                    if open {
                        return err(XmlError::RecursiveEntityRef);
                    }
                    if let Some(f) = stack.last_mut() {
                        f.pos = next;
                    }
                    if system || text.is_none() {
                        self.keep_processing = self.standalone;
                        continue;
                    }
                    if let Some(text) = text {
                        if let Some(e) = self.param.get_mut(&name) {
                            e.open = true;
                        }
                        let len = text.len();
                        stack.push(Fr {
                            src: Src::Text(text),
                            pos: 0,
                            end: len,
                            name: Some(name),
                        });
                    }
                    continue;
                }
                tok::NONE => {
                    let Some(done) = stack.pop() else {
                        break;
                    };
                    if let Some(n) = done.name
                        && let Some(e) = self.param.get_mut(&n)
                    {
                        e.open = false;
                    }
                    continue;
                }
                tok::ENTITY_REF | tok::DATA_CHARS => out.extend(enc.to_utf8(buf, p, next)),
                tok::TRAILING_CR | tok::DATA_NEWLINE => out.push(b'\n'),
                tok::CHAR_REF => {
                    let n = tk.char_ref_number(p);
                    if n < 0 {
                        if in_doc {
                            self.event = p;
                        }
                        return err(XmlError::BadCharRef);
                    }
                    crate::pystr::push_cp(&mut out, u32::try_from(n).unwrap_or(0xFFFD));
                }
                tok::PARTIAL => {
                    if in_doc {
                        self.event = p;
                    }
                    return err(XmlError::InvalidToken);
                }
                tok::INVALID => {
                    if in_doc {
                        self.event = next;
                    }
                    return err(XmlError::InvalidToken);
                }
                _ => {
                    if in_doc {
                        self.event = p;
                    }
                    return err(XmlError::UnexpectedState);
                }
            }
            let next = if t == tok::TRAILING_CR {
                p.saturating_add(enc.minbpc())
            } else {
                next
            };
            if let Some(f) = stack.last_mut() {
                f.pos = next;
            }
        }
        Ok(out)
    }

    /// `appendAttributeValue` (errors only; the normalized value is not needed): `prolog` when called from
    /// the DTD (`pool == &dtd->pool`).
    fn append_attribute_value(
        &mut self,
        src: &Src,
        s0: usize,
        end0: usize,
        prolog: bool,
        account: Account,
    ) -> Res<()> {
        struct Fr {
            src: Src,
            pos: usize,
            end: usize,
            name: Option<Vec<u8>>,
        }
        let mut stack = vec![Fr {
            src: src.clone(),
            pos: s0,
            end: end0,
            name: None,
        }];
        while let Some(fr) = stack.last_mut() {
            let in_doc = matches!(fr.src, Src::Doc);
            let enc = self.enc_of(&fr.src);
            let owned: Option<Rc<[u8]>> = match &fr.src {
                Src::Doc => None,
                Src::Text(b) => Some(Rc::clone(b)),
            };
            let p = fr.pos;
            let end = fr.end;
            let depth = stack.len();
            let buf: &[u8] = owned.as_deref().unwrap_or(self.doc);
            let tk = Tk { enc: &enc, buf };
            let (t, nx) = tk.attribute_value_tok(p, end);
            let next = nx.unwrap_or(p);
            let acc = if depth == 1 {
                account
            } else {
                Account::Expansion
            };
            self.account(t, p, next, acc)?;
            let buf: &[u8] = owned.as_deref().unwrap_or(self.doc);
            let tk = Tk { enc: &enc, buf };
            let mut next = next;
            match t {
                tok::NONE => {
                    let Some(done) = stack.pop() else {
                        break;
                    };
                    if let Some(n) = done.name
                        && let Some(e) = self.general.get_mut(&n)
                    {
                        e.open = false;
                    }
                    continue;
                }
                tok::INVALID => {
                    if in_doc {
                        self.event = next;
                    }
                    return err(XmlError::InvalidToken);
                }
                tok::PARTIAL => {
                    if in_doc {
                        self.event = p;
                    }
                    return err(XmlError::InvalidToken);
                }
                tok::CHAR_REF => {
                    if tk.char_ref_number(p) < 0 {
                        if in_doc {
                            self.event = p;
                        }
                        return err(XmlError::BadCharRef);
                    }
                }
                tok::DATA_CHARS | tok::ATTRIBUTE_VALUE_S | tok::DATA_NEWLINE => {}
                tok::TRAILING_CR => next = p.saturating_add(enc.minbpc()),
                tok::ENTITY_REF => {
                    let m = enc.minbpc();
                    if tk
                        .predefined_entity_name(p.saturating_add(m), next.saturating_sub(m))
                        .is_some()
                    {
                        self.indirect = self.indirect.saturating_add(1);
                    } else {
                        let name = enc.to_utf8(buf, p.saturating_add(m), next.saturating_sub(m));
                        let check = if prolog {
                            if self.standalone {
                                self.open_internal == 0
                            } else {
                                !self.has_param_entity_refs
                            }
                        } else {
                            !self.has_param_entity_refs || self.standalone
                        };
                        let info = self
                            .general
                            .get(&name)
                            .map(|e| (e.is_internal, e.open, e.notation, e.text.clone()));
                        match info {
                            None if check => return err(XmlError::UndefinedEntity),
                            None => {}
                            Some((false, ..)) if check => return err(XmlError::EntityDeclaredInPe),
                            Some((_, open, notation, text)) => {
                                if open {
                                    if in_doc {
                                        self.event = p;
                                    }
                                    return err(XmlError::RecursiveEntityRef);
                                }
                                if notation {
                                    if in_doc {
                                        self.event = p;
                                    }
                                    return err(XmlError::BinaryEntityRef);
                                }
                                let Some(text) = text else {
                                    if in_doc {
                                        self.event = p;
                                    }
                                    return err(XmlError::AttributeExternalEntityRef);
                                };
                                if let Some(e) = self.general.get_mut(&name) {
                                    e.open = true;
                                }
                                if let Some(f) = stack.last_mut() {
                                    f.pos = next;
                                }
                                let len = text.len();
                                stack.push(Fr {
                                    src: Src::Text(text),
                                    pos: 0,
                                    end: len,
                                    name: Some(name),
                                });
                                continue;
                            }
                        }
                    }
                }
                _ => {
                    if in_doc {
                        self.event = p;
                    }
                    return err(XmlError::UnexpectedState);
                }
            }
            if let Some(f) = stack.last_mut() {
                f.pos = next;
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------------------------------------------
    // the XML declaration and encodings
    // ------------------------------------------------------------------------------------------------------

    /// `processXmlDecl(parser, 0, s, next)`.
    #[expect(
        clippy::many_single_char_names,
        reason = "the names of expat's doParseXmlDecl"
    )]
    fn process_xml_decl(&mut self, s: usize, next: usize) -> Res<()> {
        let enc = self.enc.clone();
        let d = self.doc;
        let m = enc.minbpc();
        let mut ptr = s.saturating_add(5usize.saturating_mul(m));
        let end = next.saturating_sub(2usize.saturating_mul(m));
        let is_space = |c: i32| matches!(c, 0x20 | 0x0D | 0x0A | 0x09);
        let to_ascii = |p: usize| enc.to_ascii(d, p, end);
        let tk = Tk { enc: &enc, buf: d };
        // parsePseudoAttribute: Ok(None) = no more attributes, Ok(Some((name, name_end, val))) and the new ptr
        let parse_pseudo = |ptr: &mut usize| -> Result<Option<(usize, usize, usize)>, usize> {
            let mut p = *ptr;
            if p == end {
                return Ok(None);
            }
            if !is_space(to_ascii(p)) {
                return Err(p);
            }
            loop {
                p = p.saturating_add(m);
                if !is_space(to_ascii(p)) {
                    break;
                }
            }
            if p == end {
                return Ok(None);
            }
            let name = p;
            let name_end;
            loop {
                let c = to_ascii(p);
                if c == -1 {
                    return Err(p);
                }
                if c == i32::from(b'=') {
                    name_end = p;
                    break;
                }
                if is_space(c) {
                    name_end = p;
                    loop {
                        p = p.saturating_add(m);
                        if !is_space(to_ascii(p)) {
                            break;
                        }
                    }
                    if to_ascii(p) != i32::from(b'=') {
                        return Err(p);
                    }
                    break;
                }
                p = p.saturating_add(m);
            }
            if p == name {
                return Err(p);
            }
            p = p.saturating_add(m);
            let mut c = to_ascii(p);
            while is_space(c) {
                p = p.saturating_add(m);
                c = to_ascii(p);
            }
            if c != i32::from(b'"') && c != i32::from(b'\'') {
                return Err(p);
            }
            let open = c;
            p = p.saturating_add(m);
            let val = p;
            loop {
                let c = to_ascii(p);
                if c == open {
                    break;
                }
                let ok = (i32::from(b'a')..=i32::from(b'z')).contains(&c)
                    || (i32::from(b'A')..=i32::from(b'Z')).contains(&c)
                    || (i32::from(b'0')..=i32::from(b'9')).contains(&c)
                    || c == i32::from(b'.')
                    || c == i32::from(b'-')
                    || c == i32::from(b'_');
                if !ok {
                    return Err(p);
                }
                p = p.saturating_add(m);
            }
            *ptr = p.saturating_add(m);
            Ok(Some((name, name_end, val)))
        };
        let bad = |s: &mut Self, p: usize| {
            s.event = p;
            err(XmlError::XmlDecl)
        };
        let mut attr = match parse_pseudo(&mut ptr) {
            Ok(Some(a)) => a,
            Ok(None) => return bad(self, ptr),
            Err(p) => return bad(self, p),
        };
        if !tk.name_matches_ascii(attr.0, attr.1, b"version") {
            return bad(self, attr.0);
        }
        match parse_pseudo(&mut ptr) {
            Ok(None) => return Ok(()),
            Ok(Some(a)) => attr = a,
            Err(p) => return bad(self, p),
        }
        let mut encoding_name: Option<(usize, usize)> = None;
        if tk.name_matches_ascii(attr.0, attr.1, b"encoding") {
            let c = to_ascii(attr.2);
            if !(i32::from(b'a')..=i32::from(b'z')).contains(&c)
                && !(i32::from(b'A')..=i32::from(b'Z')).contains(&c)
            {
                return bad(self, attr.2);
            }
            encoding_name = Some((attr.2, ptr.saturating_sub(m)));
            match parse_pseudo(&mut ptr) {
                Ok(None) => return self.apply_encoding(encoding_name),
                Ok(Some(a)) => attr = a,
                Err(p) => return bad(self, p),
            }
        }
        if !tk.name_matches_ascii(attr.0, attr.1, b"standalone") {
            return bad(self, attr.0);
        }
        let val_end = ptr.saturating_sub(m);
        if tk.name_matches_ascii(attr.2, val_end, b"yes") {
            self.standalone = true;
            self.param_entity_parsing = false;
        } else if !tk.name_matches_ascii(attr.2, val_end, b"no") {
            return bad(self, attr.2);
        }
        while is_space(to_ascii(ptr)) {
            ptr = ptr.saturating_add(m);
        }
        if ptr != end {
            return bad(self, ptr);
        }
        self.apply_encoding(encoding_name)
    }

    /// The encoding part of `processXmlDecl`: `findEncoding`, the conflict check and `handleUnknownEncoding`.
    fn apply_encoding(&mut self, name: Option<(usize, usize)>) -> Res<()> {
        let Some((a, b)) = name else {
            return Ok(());
        };
        let raw = self.enc.to_utf8(self.doc, a, b);
        let new = if raw.len() < 128 {
            if raw.eq_ignore_ascii_case(b"UTF-16") && self.enc.minbpc() == 2 {
                Some(self.enc.clone())
            } else {
                enc::builtin(&raw).map(|bi| match bi {
                    Builtin::Latin1 => Enc::Latin1,
                    Builtin::Ascii => Enc::Ascii,
                    Builtin::Utf8 => Enc::Utf8,
                    Builtin::Utf16 | Builtin::Utf16Be => Enc::Big2,
                    Builtin::Utf16Le => Enc::Little2,
                })
            }
        } else {
            None
        };
        if let Some(new) = new {
            if new.minbpc() != self.enc.minbpc() || new.minbpc() == 2 && new != self.enc {
                self.event = a;
                return err(XmlError::IncorrectEncoding);
            }
            self.enc = new;
            return Ok(());
        }
        // XmlNameLength from the start of the name (the value's characters are name characters)
        let tk = Tk {
            enc: &self.enc,
            buf: self.doc,
        };
        let len = tk.name_length(a);
        let stored = self.enc.to_utf8(self.doc, a, a.saturating_add(len));
        match unknown_encoding_handler(&stored, self.minor) {
            Err(f) => Err(Stop::Py(f)),
            Ok(None) => {
                self.event = a;
                err(XmlError::UnknownEncoding)
            }
            Ok(Some(map)) => {
                if let Some(e) = enc::unknown_encoding(&map) {
                    self.enc = e;
                    Ok(())
                } else {
                    self.event = a;
                    err(XmlError::UnknownEncoding)
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------------------------
    // content
    // ------------------------------------------------------------------------------------------------------

    /// `contentProcessor` → `doContent(parser, 0, …)` with internal entities on a stack.
    fn do_content(&mut self, s0: usize) -> Res<()> {
        struct Fr {
            src: Src,
            pos: usize,
            end: usize,
            start_tag_level: usize,
            name: Vec<u8>,
        }
        let mut stack: Vec<Fr> = Vec::new();
        let mut src = Src::Doc;
        let mut s = s0;
        let mut end = self.doc.len();
        let mut start_tag_level = 0usize;
        if matches!(src, Src::Doc) {
            self.event = s;
        }
        loop {
            let in_doc = stack.is_empty();
            let enc = self.enc_of(&src);
            let owned: Option<Rc<[u8]>> = match &src {
                Src::Doc => None,
                Src::Text(b) => Some(Rc::clone(b)),
            };
            let buf: &[u8] = owned.as_deref().unwrap_or(self.doc);
            let (t, nx) = Tk { enc: &enc, buf }.content_tok(s, end);
            let next = nx.unwrap_or(s);
            self.account(
                t,
                s,
                next,
                if in_doc {
                    Account::Direct
                } else {
                    Account::Expansion
                },
            )?;
            let buf: &[u8] = owned.as_deref().unwrap_or(self.doc);
            let tk = Tk { enc: &enc, buf };
            let m = enc.minbpc();
            match t {
                tok::TRAILING_CR | tok::NONE | tok::TRAILING_RSQB => {
                    if t != tok::NONE && in_doc {
                        self.event = end;
                    }
                    if start_tag_level == 0 {
                        if t == tok::TRAILING_RSQB && in_doc {
                            self.event = end;
                        }
                        return err(XmlError::NoElements);
                    }
                    if self.tags.len() != start_tag_level {
                        return err(XmlError::AsyncEntity);
                    }
                    // the end of an internal entity
                    let Some(f) = stack.pop() else {
                        return err(XmlError::UnexpectedState);
                    };
                    if let Some(e) = self.general.get_mut(&f.name) {
                        e.open = false;
                    }
                    self.open_internal = self.open_internal.saturating_sub(1);
                    src = f.src;
                    s = f.pos;
                    end = f.end;
                    start_tag_level = f.start_tag_level;
                    if stack.is_empty() {
                        self.event = s;
                    }
                    continue;
                }
                tok::INVALID => {
                    if in_doc {
                        self.event = next;
                    }
                    return err(XmlError::InvalidToken);
                }
                tok::PARTIAL => return err(XmlError::UnclosedToken),
                tok::PARTIAL_CHAR => return err(XmlError::PartialChar),
                tok::ENTITY_REF => {
                    if tk
                        .predefined_entity_name(s.saturating_add(m), next.saturating_sub(m))
                        .is_some()
                    {
                        self.indirect = self.indirect.saturating_add(1);
                    } else {
                        let name = enc.to_utf8(buf, s.saturating_add(m), next.saturating_sub(m));
                        let info = self
                            .general
                            .get(&name)
                            .map(|e| (e.is_internal, e.open, e.notation, e.text.clone()));
                        let check = !self.has_param_entity_refs || self.standalone;
                        match info {
                            None if check => return err(XmlError::UndefinedEntity),
                            None => {}
                            Some((false, ..)) if check => return err(XmlError::EntityDeclaredInPe),
                            Some((_, open, notation, text)) => {
                                if open {
                                    return err(XmlError::RecursiveEntityRef);
                                }
                                if notation {
                                    return err(XmlError::BinaryEntityRef);
                                }
                                if let Some(text) = text {
                                    if let Some(e) = self.general.get_mut(&name) {
                                        e.open = true;
                                    }
                                    self.open_internal = self.open_internal.saturating_add(1);
                                    stack.push(Fr {
                                        src: src.clone(),
                                        pos: next,
                                        end,
                                        start_tag_level,
                                        name,
                                    });
                                    start_tag_level = self.tags.len();
                                    end = text.len();
                                    src = Src::Text(text);
                                    s = 0;
                                    continue;
                                }
                                // an external entity: pyexpat's handler returns 1 without parsing it
                            }
                        }
                    }
                }
                tok::START_TAG_NO_ATTS | tok::START_TAG_WITH_ATTS => {
                    let raw = s.saturating_add(m);
                    let len = tk.name_length(raw);
                    let name = buf
                        .get(raw..raw.saturating_add(len))
                        .unwrap_or_default()
                        .to_vec();
                    self.tags.push((name, in_doc));
                    self.store_atts(
                        &src,
                        s,
                        if in_doc {
                            Account::Direct
                        } else {
                            Account::Expansion
                        },
                    )?;
                }
                tok::EMPTY_ELEMENT_NO_ATTS | tok::EMPTY_ELEMENT_WITH_ATTS => {
                    self.store_atts(&src, s, Account::None)?;
                    if self.tags.is_empty() {
                        return self.epilog(next);
                    }
                }
                tok::END_TAG => {
                    if self.tags.len() == start_tag_level {
                        return err(XmlError::AsyncEntity);
                    }
                    let raw = s.saturating_add(2usize.saturating_mul(m));
                    let len = tk.name_length(raw);
                    let name = buf.get(raw..raw.saturating_add(len)).unwrap_or_default();
                    let matches = self.tags.last().is_some_and(|(n, _)| n.as_slice() == name);
                    if !matches {
                        if in_doc {
                            self.event = raw;
                        }
                        return err(XmlError::TagMismatch);
                    }
                    self.tags.pop();
                    if self.tags.is_empty() {
                        return self.epilog(next);
                    }
                }
                tok::CHAR_REF => {
                    if tk.char_ref_number(s) < 0 {
                        return err(XmlError::BadCharRef);
                    }
                }
                tok::XML_DECL => return err(XmlError::MisplacedXmlPi),
                tok::CDATA_SECT_OPEN => {
                    let mut p = next;
                    if in_doc {
                        self.event = p;
                    }
                    loop {
                        let (ct, cn) = tk.cdata_section_tok(p, end);
                        let cnext = cn.unwrap_or(p);
                        self.account(
                            ct,
                            p,
                            cnext,
                            if in_doc {
                                Account::Direct
                            } else {
                                Account::Expansion
                            },
                        )?;
                        match ct {
                            tok::CDATA_SECT_CLOSE => {
                                p = cnext;
                                break;
                            }
                            tok::DATA_NEWLINE | tok::DATA_CHARS => {}
                            tok::INVALID => {
                                if in_doc {
                                    self.event = cnext;
                                }
                                return err(XmlError::InvalidToken);
                            }
                            tok::PARTIAL_CHAR => return err(XmlError::PartialChar),
                            tok::PARTIAL | tok::NONE => return err(XmlError::UnclosedCdataSection),
                            _ => {
                                if in_doc {
                                    self.event = cnext;
                                }
                                return err(XmlError::UnexpectedState);
                            }
                        }
                        p = cnext;
                        if in_doc {
                            self.event = p;
                        }
                    }
                    s = p;
                    if in_doc {
                        self.event = s;
                    }
                    continue;
                }
                _ => {}
            }
            s = next;
            if in_doc {
                self.event = s;
            }
        }
    }

    /// `storeAtts`: attribute names must be unique, and values are normalized (entity references checked).
    fn store_atts(&mut self, src: &Src, s: usize, account: Account) -> Res<()> {
        let enc = self.enc_of(src);
        let owned: Option<Rc<[u8]>> = match src {
            Src::Doc => None,
            Src::Text(b) => Some(Rc::clone(b)),
        };
        let buf: &[u8] = owned.as_deref().unwrap_or(self.doc);
        let tk = Tk { enc: &enc, buf };
        let atts = tk.get_atts(s);
        let mut seen: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
        for a in atts {
            let len = tk.name_length(a.name);
            let name = enc.to_utf8(buf, a.name, a.name.saturating_add(len));
            if !seen.insert(name) {
                if matches!(src, Src::Doc) {
                    self.event = a.name;
                }
                return err(XmlError::DuplicateAttribute);
            }
            if !a.normalized {
                self.append_attribute_value(src, a.value, a.value_end, false, account)?;
            }
        }
        Ok(())
    }

    /// `epilogProcessor`.
    fn epilog(&mut self, s0: usize) -> Res<()> {
        let mut s = s0;
        self.event = s;
        let end = self.doc.len();
        loop {
            let enc = self.enc.clone();
            let (t, nx) = Tk {
                enc: &enc,
                buf: self.doc,
            }
            .prolog_tok(s, end);
            let next = nx.unwrap_or(s);
            self.account(t, s, next, Account::Direct)?;
            match t {
                t2 if t2 == -tok::PROLOG_S => return Ok(()),
                tok::NONE => return Ok(()),
                tok::PROLOG_S | tok::PI | tok::COMMENT => {}
                tok::INVALID => {
                    self.event = next;
                    return err(XmlError::InvalidToken);
                }
                tok::PARTIAL => return err(XmlError::UnclosedToken),
                tok::PARTIAL_CHAR => return err(XmlError::PartialChar),
                _ => return err(XmlError::JunkAfterDocElement),
            }
            s = next;
            self.event = s;
        }
    }

    /// `XmlUpdatePosition` from the start of the document to `p` in the current encoding: (line, column),
    /// both 0-based.
    pub(super) fn position(&self, p: usize) -> (usize, usize) {
        let enc = &self.enc;
        let m = enc.minbpc();
        let d = self.doc;
        let end = p.min(d.len());
        let (mut line, mut col) = (0usize, 0usize);
        let mut i = 0usize;
        while i.saturating_add(m) <= end {
            match enc.bt(d, i) {
                enc::Bt::Lead2 => {
                    i = i.saturating_add(2);
                    col = col.saturating_add(1);
                }
                enc::Bt::Lead3 => {
                    i = i.saturating_add(3);
                    col = col.saturating_add(1);
                }
                enc::Bt::Lead4 => {
                    i = i.saturating_add(4);
                    col = col.saturating_add(1);
                }
                enc::Bt::Lf => {
                    col = 0;
                    line = line.saturating_add(1);
                    i = i.saturating_add(m);
                }
                enc::Bt::Cr => {
                    line = line.saturating_add(1);
                    i = i.saturating_add(m);
                    if i.saturating_add(m) <= end && enc.bt(d, i) == enc::Bt::Lf {
                        i = i.saturating_add(m);
                    }
                    col = 0;
                }
                _ => {
                    i = i.saturating_add(m);
                    col = col.saturating_add(1);
                }
            }
        }
        (line, col)
    }
}

/// `_Py_normalize_encoding`: ASCII lower case, runs of other characters (except `.`) as one `_`.
fn normalize_encoding(name: &[u8]) -> String {
    let mut out = String::new();
    let mut punct = false;
    for &c in name {
        if c.is_ascii_alphanumeric() || c == b'.' {
            if punct && !out.is_empty() {
                out.push('_');
            }
            out.push(char::from(c.to_ascii_lowercase()));
            punct = false;
        } else {
            punct = true;
        }
    }
    out
}

/// pyexpat's `PyUnknownEncodingHandler`: `bytes(range(256)).decode(name, 'replace')` through Python's codec
/// registry. `Ok(None)`: the handler fails without a Python exception (never for pyexpat); `Err`: the exception
/// it leaves set, which pyexpat raises after `XML_Parse` fails.
fn unknown_encoding_handler(name: &[u8], minor: PyMinor) -> Result<Option<[i32; 256]>, Fail> {
    let display = String::from_utf8_lossy(name).into_owned();
    let norm = normalize_encoding(name);
    let (modules, aliases, tables) = super::codec_tables(minor);
    let alias = aliases
        .iter()
        .find(|(a, _)| *a == norm)
        .or_else(|| {
            let dotless = norm.replace('.', "_");
            aliases.iter().find(|(a, _)| *a == dotless)
        })
        .map(|(_, t)| *t);
    let mut candidates: Vec<&str> = Vec::new();
    if let Some(a) = alias {
        candidates.push(a);
    }
    candidates.push(&norm);
    for c in candidates {
        if c.is_empty() || c.contains('.') {
            continue;
        }
        if let Some((_, codec)) = modules.iter().find(|(m, _)| *m == c) {
            return match codec {
                Codec::Table(i) => Ok(tables.get(*i).copied()),
                Codec::MultiByte => Err(Fail::Crash(
                    "ValueError",
                    "multi-byte encodings are not supported".into(),
                )),
                Codec::Error(ty, text) => {
                    if *ty == "LookupError" {
                        Err(Fail::Crash(
                            "LookupError",
                            format!(
                                "'{display}' is not a text encoding; use codecs.decode() to handle arbitrary codecs"
                            ),
                        ))
                    } else {
                        Err(Fail::Crash(ty, (*text).to_owned()))
                    }
                }
            };
        }
    }
    Err(Fail::Crash(
        "LookupError",
        format!("unknown encoding: {display}"),
    ))
}
