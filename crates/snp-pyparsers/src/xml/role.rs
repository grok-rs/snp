//! expat's prolog grammar (`xmlrole.c`): the `PROLOG_STATE` handlers that turn prolog tokens into roles.
//!
//! Owner: WP-17.

use super::tok::{self, Tk};

/// `XML_ROLE_*`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Role {
    Error,
    None,
    XmlDecl,
    InstanceStart,
    DoctypeNone,
    DoctypeName,
    DoctypeSystemId,
    DoctypePublicId,
    DoctypeInternalSubset,
    DoctypeClose,
    GeneralEntityName,
    ParamEntityName,
    EntityNone,
    EntityValue,
    EntitySystemId,
    EntityPublicId,
    EntityComplete,
    EntityNotationName,
    NotationNone,
    NotationName,
    NotationSystemId,
    NotationNoSystemId,
    NotationPublicId,
    AttributeName,
    /// `XML_ROLE_ATTRIBUTE_TYPE_CDATA + i` (CDATA, ID, IDREF, IDREFS, ENTITY, ENTITIES, NMTOKEN, NMTOKENS).
    AttributeType(u8),
    AttributeEnumValue,
    AttributeNotationValue,
    AttlistNone,
    AttlistElementName,
    ImpliedAttributeValue,
    RequiredAttributeValue,
    DefaultAttributeValue,
    FixedAttributeValue,
    ElementNone,
    ElementName,
    ContentAny,
    ContentEmpty,
    ContentPcdata,
    GroupOpen,
    GroupClose,
    GroupCloseRep,
    GroupCloseOpt,
    GroupClosePlus,
    GroupChoice,
    GroupSequence,
    ContentElement,
    ContentElementRep,
    ContentElementOpt,
    ContentElementPlus,
    Pi,
    Comment,
    ParamEntityRef,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum H {
    Prolog0,
    Prolog1,
    Prolog2,
    Doctype0,
    Doctype1,
    Doctype2,
    Doctype3,
    Doctype4,
    Doctype5,
    InternalSubset,
    Entity0,
    Entity1,
    Entity2,
    Entity3,
    Entity4,
    Entity5,
    Entity6,
    Entity7,
    Entity8,
    Entity9,
    Entity10,
    Notation0,
    Notation1,
    Notation2,
    Notation3,
    Notation4,
    Attlist0,
    Attlist1,
    Attlist2,
    Attlist3,
    Attlist4,
    Attlist5,
    Attlist6,
    Attlist7,
    Attlist8,
    Attlist9,
    Element0,
    Element1,
    Element2,
    Element3,
    Element4,
    Element5,
    Element6,
    Element7,
    DeclClose,
    Error,
}

/// `PROLOG_STATE` of the document entity.
#[derive(Debug)]
pub(super) struct PrologState {
    handler: H,
    level: u32,
    role_none: Role,
}

impl Default for PrologState {
    fn default() -> Self {
        Self {
            handler: H::Prolog0,
            level: 0,
            role_none: Role::None,
        }
    }
}

impl PrologState {
    /// `state->level` (the content-model group depth).
    pub(super) const fn level(&self) -> u32 {
        self.level
    }

    /// `common(state, tok)` (the document entity: a parameter-entity reference is never "inner").
    fn common(&mut self, _t: i32) -> Role {
        self.handler = H::Error;
        Role::Error
    }

    fn set_top_level(&mut self) {
        self.handler = H::InternalSubset;
    }

    /// `XmlTokenRole(state, tok, ptr, end, enc)`.
    pub(super) fn token_role(&mut self, t: i32, p: usize, end: usize, tk: &Tk<'_>) -> Role {
        let m = tk.enc.minbpc();
        let kw = |k: &[u8], from: usize| tk.name_matches_ascii(from, end, k);
        match self.handler {
            H::Prolog0 | H::Prolog1 => match t {
                tok::PROLOG_S => {
                    self.handler = H::Prolog1;
                    Role::None
                }
                tok::XML_DECL if self.handler == H::Prolog0 => {
                    self.handler = H::Prolog1;
                    Role::XmlDecl
                }
                tok::PI => {
                    self.handler = H::Prolog1;
                    Role::Pi
                }
                tok::COMMENT => {
                    self.handler = H::Prolog1;
                    Role::Comment
                }
                tok::BOM => Role::None,
                tok::DECL_OPEN if kw(b"DOCTYPE", p.saturating_add(2usize.saturating_mul(m))) => {
                    self.handler = H::Doctype0;
                    Role::DoctypeNone
                }
                tok::INSTANCE_START => {
                    self.handler = H::Error;
                    Role::InstanceStart
                }
                _ => self.common(t),
            },
            H::Prolog2 => match t {
                tok::PROLOG_S => Role::None,
                tok::PI => Role::Pi,
                tok::COMMENT => Role::Comment,
                tok::INSTANCE_START => {
                    self.handler = H::Error;
                    Role::InstanceStart
                }
                _ => self.common(t),
            },
            H::Doctype0 => match t {
                tok::PROLOG_S => Role::DoctypeNone,
                tok::NAME => {
                    self.handler = H::Doctype1;
                    Role::DoctypeName
                }
                _ => self.common(t),
            },
            H::Doctype1 => match t {
                tok::PROLOG_S => Role::DoctypeNone,
                tok::OPEN_BRACKET => {
                    self.handler = H::InternalSubset;
                    Role::DoctypeInternalSubset
                }
                tok::DECL_CLOSE => {
                    self.handler = H::Prolog2;
                    Role::DoctypeClose
                }
                tok::NAME if kw(b"SYSTEM", p) => {
                    self.handler = H::Doctype3;
                    Role::DoctypeNone
                }
                tok::NAME if kw(b"PUBLIC", p) => {
                    self.handler = H::Doctype2;
                    Role::DoctypeNone
                }
                _ => self.common(t),
            },
            H::Doctype2 => match t {
                tok::PROLOG_S => Role::DoctypeNone,
                tok::LITERAL => {
                    self.handler = H::Doctype3;
                    Role::DoctypePublicId
                }
                _ => self.common(t),
            },
            H::Doctype3 => match t {
                tok::PROLOG_S => Role::DoctypeNone,
                tok::LITERAL => {
                    self.handler = H::Doctype4;
                    Role::DoctypeSystemId
                }
                _ => self.common(t),
            },
            H::Doctype4 => match t {
                tok::PROLOG_S => Role::DoctypeNone,
                tok::OPEN_BRACKET => {
                    self.handler = H::InternalSubset;
                    Role::DoctypeInternalSubset
                }
                tok::DECL_CLOSE => {
                    self.handler = H::Prolog2;
                    Role::DoctypeClose
                }
                _ => self.common(t),
            },
            H::Doctype5 => match t {
                tok::PROLOG_S => Role::DoctypeNone,
                tok::DECL_CLOSE => {
                    self.handler = H::Prolog2;
                    Role::DoctypeClose
                }
                _ => self.common(t),
            },
            H::InternalSubset => match t {
                tok::PROLOG_S | tok::NONE => Role::None,
                tok::DECL_OPEN if kw(b"ENTITY", p.saturating_add(2usize.saturating_mul(m))) => {
                    self.handler = H::Entity0;
                    Role::EntityNone
                }
                tok::DECL_OPEN if kw(b"ATTLIST", p.saturating_add(2usize.saturating_mul(m))) => {
                    self.handler = H::Attlist0;
                    Role::AttlistNone
                }
                tok::DECL_OPEN if kw(b"ELEMENT", p.saturating_add(2usize.saturating_mul(m))) => {
                    self.handler = H::Element0;
                    Role::ElementNone
                }
                tok::DECL_OPEN if kw(b"NOTATION", p.saturating_add(2usize.saturating_mul(m))) => {
                    self.handler = H::Notation0;
                    Role::NotationNone
                }
                tok::PI => Role::Pi,
                tok::COMMENT => Role::Comment,
                tok::PARAM_ENTITY_REF => Role::ParamEntityRef,
                tok::CLOSE_BRACKET => {
                    self.handler = H::Doctype5;
                    Role::DoctypeNone
                }
                _ => self.common(t),
            },
            H::Error => Role::None,
            H::Entity0 => match t {
                tok::PROLOG_S => Role::EntityNone,
                tok::PERCENT => {
                    self.handler = H::Entity1;
                    Role::EntityNone
                }
                tok::NAME => {
                    self.handler = H::Entity2;
                    Role::GeneralEntityName
                }
                _ => self.common(t),
            },
            H::Entity1 => match t {
                tok::PROLOG_S => Role::EntityNone,
                tok::NAME => {
                    self.handler = H::Entity7;
                    Role::ParamEntityName
                }
                _ => self.common(t),
            },
            H::Entity2 | H::Entity7 => {
                let general = self.handler == H::Entity2;
                match t {
                    tok::PROLOG_S => Role::EntityNone,
                    tok::NAME if kw(b"SYSTEM", p) => {
                        self.handler = if general { H::Entity4 } else { H::Entity9 };
                        Role::EntityNone
                    }
                    tok::NAME if kw(b"PUBLIC", p) => {
                        self.handler = if general { H::Entity3 } else { H::Entity8 };
                        Role::EntityNone
                    }
                    tok::LITERAL => {
                        self.handler = H::DeclClose;
                        self.role_none = Role::EntityNone;
                        Role::EntityValue
                    }
                    _ => self.common(t),
                }
            }
            H::Entity3 | H::Entity8 => match t {
                tok::PROLOG_S => Role::EntityNone,
                tok::LITERAL => {
                    self.handler = if self.handler == H::Entity3 {
                        H::Entity4
                    } else {
                        H::Entity9
                    };
                    Role::EntityPublicId
                }
                _ => self.common(t),
            },
            H::Entity4 | H::Entity9 => match t {
                tok::PROLOG_S => Role::EntityNone,
                tok::LITERAL => {
                    self.handler = if self.handler == H::Entity4 {
                        H::Entity5
                    } else {
                        H::Entity10
                    };
                    Role::EntitySystemId
                }
                _ => self.common(t),
            },
            H::Entity5 => match t {
                tok::PROLOG_S => Role::EntityNone,
                tok::DECL_CLOSE => {
                    self.set_top_level();
                    Role::EntityComplete
                }
                tok::NAME if kw(b"NDATA", p) => {
                    self.handler = H::Entity6;
                    Role::EntityNone
                }
                _ => self.common(t),
            },
            H::Entity6 => match t {
                tok::PROLOG_S => Role::EntityNone,
                tok::NAME => {
                    self.handler = H::DeclClose;
                    self.role_none = Role::EntityNone;
                    Role::EntityNotationName
                }
                _ => self.common(t),
            },
            H::Entity10 => match t {
                tok::PROLOG_S => Role::EntityNone,
                tok::DECL_CLOSE => {
                    self.set_top_level();
                    Role::EntityComplete
                }
                _ => self.common(t),
            },
            H::Notation0 => match t {
                tok::PROLOG_S => Role::NotationNone,
                tok::NAME => {
                    self.handler = H::Notation1;
                    Role::NotationName
                }
                _ => self.common(t),
            },
            H::Notation1 => match t {
                tok::PROLOG_S => Role::NotationNone,
                tok::NAME if kw(b"SYSTEM", p) => {
                    self.handler = H::Notation3;
                    Role::NotationNone
                }
                tok::NAME if kw(b"PUBLIC", p) => {
                    self.handler = H::Notation2;
                    Role::NotationNone
                }
                _ => self.common(t),
            },
            H::Notation2 => match t {
                tok::PROLOG_S => Role::NotationNone,
                tok::LITERAL => {
                    self.handler = H::Notation4;
                    Role::NotationPublicId
                }
                _ => self.common(t),
            },
            H::Notation3 => match t {
                tok::PROLOG_S => Role::NotationNone,
                tok::LITERAL => {
                    self.handler = H::DeclClose;
                    self.role_none = Role::NotationNone;
                    Role::NotationSystemId
                }
                _ => self.common(t),
            },
            H::Notation4 => match t {
                tok::PROLOG_S => Role::NotationNone,
                tok::LITERAL => {
                    self.handler = H::DeclClose;
                    self.role_none = Role::NotationNone;
                    Role::NotationSystemId
                }
                tok::DECL_CLOSE => {
                    self.set_top_level();
                    Role::NotationNoSystemId
                }
                _ => self.common(t),
            },
            H::Attlist0 => match t {
                tok::PROLOG_S => Role::AttlistNone,
                tok::NAME => {
                    self.handler = H::Attlist1;
                    Role::AttlistElementName
                }
                _ => self.common(t),
            },
            H::Attlist1 => match t {
                tok::PROLOG_S => Role::AttlistNone,
                tok::DECL_CLOSE => {
                    self.set_top_level();
                    Role::AttlistNone
                }
                tok::NAME => {
                    self.handler = H::Attlist2;
                    Role::AttributeName
                }
                _ => self.common(t),
            },
            H::Attlist2 => match t {
                tok::PROLOG_S => Role::AttlistNone,
                tok::NAME => {
                    let types: [&[u8]; 8] = [
                        b"CDATA",
                        b"ID",
                        b"IDREF",
                        b"IDREFS",
                        b"ENTITY",
                        b"ENTITIES",
                        b"NMTOKEN",
                        b"NMTOKENS",
                    ];
                    for (i, k) in types.iter().enumerate() {
                        if kw(k, p) {
                            self.handler = H::Attlist8;
                            return Role::AttributeType(u8::try_from(i).unwrap_or(0));
                        }
                    }
                    if kw(b"NOTATION", p) {
                        self.handler = H::Attlist5;
                        return Role::AttlistNone;
                    }
                    self.common(t)
                }
                tok::OPEN_PAREN => {
                    self.handler = H::Attlist3;
                    Role::AttlistNone
                }
                _ => self.common(t),
            },
            H::Attlist3 => match t {
                tok::PROLOG_S => Role::AttlistNone,
                tok::NMTOKEN | tok::NAME => {
                    self.handler = H::Attlist4;
                    Role::AttributeEnumValue
                }
                _ => self.common(t),
            },
            H::Attlist4 => match t {
                tok::PROLOG_S => Role::AttlistNone,
                tok::CLOSE_PAREN => {
                    self.handler = H::Attlist8;
                    Role::AttlistNone
                }
                tok::OR => {
                    self.handler = H::Attlist3;
                    Role::AttlistNone
                }
                _ => self.common(t),
            },
            H::Attlist5 => match t {
                tok::PROLOG_S => Role::AttlistNone,
                tok::OPEN_PAREN => {
                    self.handler = H::Attlist6;
                    Role::AttlistNone
                }
                _ => self.common(t),
            },
            H::Attlist6 => match t {
                tok::PROLOG_S => Role::AttlistNone,
                tok::NAME => {
                    self.handler = H::Attlist7;
                    Role::AttributeNotationValue
                }
                _ => self.common(t),
            },
            H::Attlist7 => match t {
                tok::PROLOG_S => Role::AttlistNone,
                tok::CLOSE_PAREN => {
                    self.handler = H::Attlist8;
                    Role::AttlistNone
                }
                tok::OR => {
                    self.handler = H::Attlist6;
                    Role::AttlistNone
                }
                _ => self.common(t),
            },
            H::Attlist8 => match t {
                tok::PROLOG_S => Role::AttlistNone,
                tok::POUND_NAME if kw(b"IMPLIED", p.saturating_add(m)) => {
                    self.handler = H::Attlist1;
                    Role::ImpliedAttributeValue
                }
                tok::POUND_NAME if kw(b"REQUIRED", p.saturating_add(m)) => {
                    self.handler = H::Attlist1;
                    Role::RequiredAttributeValue
                }
                tok::POUND_NAME if kw(b"FIXED", p.saturating_add(m)) => {
                    self.handler = H::Attlist9;
                    Role::AttlistNone
                }
                tok::LITERAL => {
                    self.handler = H::Attlist1;
                    Role::DefaultAttributeValue
                }
                _ => self.common(t),
            },
            H::Attlist9 => match t {
                tok::PROLOG_S => Role::AttlistNone,
                tok::LITERAL => {
                    self.handler = H::Attlist1;
                    Role::FixedAttributeValue
                }
                _ => self.common(t),
            },
            H::Element0 => match t {
                tok::PROLOG_S => Role::ElementNone,
                tok::NAME => {
                    self.handler = H::Element1;
                    Role::ElementName
                }
                _ => self.common(t),
            },
            H::Element1 => match t {
                tok::PROLOG_S => Role::ElementNone,
                tok::NAME if kw(b"EMPTY", p) => {
                    self.handler = H::DeclClose;
                    self.role_none = Role::ElementNone;
                    Role::ContentEmpty
                }
                tok::NAME if kw(b"ANY", p) => {
                    self.handler = H::DeclClose;
                    self.role_none = Role::ElementNone;
                    Role::ContentAny
                }
                tok::OPEN_PAREN => {
                    self.handler = H::Element2;
                    self.level = 1;
                    Role::GroupOpen
                }
                _ => self.common(t),
            },
            H::Element2 => match t {
                tok::PROLOG_S => Role::ElementNone,
                tok::POUND_NAME if kw(b"PCDATA", p.saturating_add(m)) => {
                    self.handler = H::Element3;
                    Role::ContentPcdata
                }
                tok::OPEN_PAREN => {
                    self.level = 2;
                    self.handler = H::Element6;
                    Role::GroupOpen
                }
                tok::NAME => {
                    self.handler = H::Element7;
                    Role::ContentElement
                }
                tok::NAME_QUESTION => {
                    self.handler = H::Element7;
                    Role::ContentElementOpt
                }
                tok::NAME_ASTERISK => {
                    self.handler = H::Element7;
                    Role::ContentElementRep
                }
                tok::NAME_PLUS => {
                    self.handler = H::Element7;
                    Role::ContentElementPlus
                }
                _ => self.common(t),
            },
            H::Element3 => match t {
                tok::PROLOG_S => Role::ElementNone,
                tok::CLOSE_PAREN => {
                    self.handler = H::DeclClose;
                    self.role_none = Role::ElementNone;
                    Role::GroupClose
                }
                tok::CLOSE_PAREN_ASTERISK => {
                    self.handler = H::DeclClose;
                    self.role_none = Role::ElementNone;
                    Role::GroupCloseRep
                }
                tok::OR => {
                    self.handler = H::Element4;
                    Role::ElementNone
                }
                _ => self.common(t),
            },
            H::Element4 => match t {
                tok::PROLOG_S => Role::ElementNone,
                tok::NAME => {
                    self.handler = H::Element5;
                    Role::ContentElement
                }
                _ => self.common(t),
            },
            H::Element5 => match t {
                tok::PROLOG_S => Role::ElementNone,
                tok::CLOSE_PAREN_ASTERISK => {
                    self.handler = H::DeclClose;
                    self.role_none = Role::ElementNone;
                    Role::GroupCloseRep
                }
                tok::OR => {
                    self.handler = H::Element4;
                    Role::ElementNone
                }
                _ => self.common(t),
            },
            H::Element6 => match t {
                tok::PROLOG_S => Role::ElementNone,
                tok::OPEN_PAREN => {
                    self.level = self.level.saturating_add(1);
                    Role::GroupOpen
                }
                tok::NAME => {
                    self.handler = H::Element7;
                    Role::ContentElement
                }
                tok::NAME_QUESTION => {
                    self.handler = H::Element7;
                    Role::ContentElementOpt
                }
                tok::NAME_ASTERISK => {
                    self.handler = H::Element7;
                    Role::ContentElementRep
                }
                tok::NAME_PLUS => {
                    self.handler = H::Element7;
                    Role::ContentElementPlus
                }
                _ => self.common(t),
            },
            H::Element7 => {
                let close = |s: &mut Self, r: Role| {
                    s.level = s.level.saturating_sub(1);
                    if s.level == 0 {
                        s.handler = H::DeclClose;
                        s.role_none = Role::ElementNone;
                    }
                    r
                };
                match t {
                    tok::PROLOG_S => Role::ElementNone,
                    tok::CLOSE_PAREN => close(self, Role::GroupClose),
                    tok::CLOSE_PAREN_ASTERISK => close(self, Role::GroupCloseRep),
                    tok::CLOSE_PAREN_QUESTION => close(self, Role::GroupCloseOpt),
                    tok::CLOSE_PAREN_PLUS => close(self, Role::GroupClosePlus),
                    tok::COMMA => {
                        self.handler = H::Element6;
                        Role::GroupSequence
                    }
                    tok::OR => {
                        self.handler = H::Element6;
                        Role::GroupChoice
                    }
                    _ => self.common(t),
                }
            }
            H::DeclClose => match t {
                tok::PROLOG_S => self.role_none,
                tok::DECL_CLOSE => {
                    self.set_top_level();
                    self.role_none
                }
                _ => self.common(t),
            },
        }
    }
}
