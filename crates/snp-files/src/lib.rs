//! File table (one byte arena) with tiered lazy identify tags, the classifier and hook filters (ARCHITECTURE
//! §1.4.17, §2.3, ADR-079; pre-commit's `commands/run.py:Classifier`, `identify.tags_from_path`).
//!
//! Owner: WP-22.

mod classifier;
mod filter;
mod messages;
mod table;

pub use classifier::Classifier;
pub use filter::HookFilter;
pub use table::{FileId, FileTable, Needs, TagOutcome};
