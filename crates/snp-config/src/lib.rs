//! pre-commit config and manifest schemas on cfgv 0.3, the typed model, hook merging, extension keys, the local
//! overlay, migrate-config, the autoupdate rev-line rewrite, validate-*, the sample config and the JSON schema
//! (ARCHITECTURE §1.4.15; 01 §2–§11; `clientlib.py`, `repository.py`, `commands/migrate_config.py`).
//!
//! Owner: WP-21.

mod convert;
pub mod discover;
mod dump;
pub mod ext;
mod items;
mod load;
mod merge;
mod messages;
pub mod migrate;
mod model;
pub mod overlay;
pub mod revline;
mod sample;
pub mod schema;
pub mod validate;

pub use dump::{effective_dump, json_schema};
pub use load::{load_config, load_manifest, load_manifest_bytes};
pub use merge::{finalize_local, merge_hook};
pub use model::{
    Config, ConfigError, ConfigHook, ConfigSource, ExtMode, HookDef, LoadOpts, Manifest,
    MergeError, RepoConfig,
};
pub use sample::SAMPLE_CONFIG;
