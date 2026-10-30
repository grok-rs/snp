//! The event adapter: the composer's only view of the YAML parser (the ADR-002 seam). The parser is snp's
//! panic-free port of libyaml 0.2.5 ([`crate::libyaml`]), the fork of libyaml-safer 0.3.0 that ADR-002 provides
//! for; swapping the engine means re-implementing this module only.
//!
//! Owner: WP-14.

use crate::libyaml::parser::Parser;

pub(crate) use crate::libyaml::parser::{Event, EventKind};
pub(crate) use crate::libyaml::{Error as ParseError, Mark as RawMark, Style};

/// The event stream of one text.
#[derive(Debug)]
pub(crate) struct Events<'a> {
    parser: Parser<'a>,
}

impl<'a> Events<'a> {
    /// Starts parsing `text` (a leading BOM is skipped, as libyaml does).
    pub(crate) fn new(text: &'a str) -> Self {
        Self {
            parser: Parser::new(text),
        }
    }

    /// The next event (`yaml_parser_parse`); after STREAM-END it keeps returning STREAM-END.
    pub(crate) fn next_event(&mut self) -> Result<Event, ParseError> {
        self.parser.next_event()
    }
}
