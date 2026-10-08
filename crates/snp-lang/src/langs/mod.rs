//! One zero-sized backend per language, registered in [`crate::backend`] (§6.2). W0 stubs, implemented by WP-16
//! (`envless`), WP-24 (group 1) and WP-25 (group 2).
//!
//! Owner: WP-16.

pub mod conda;
pub mod coursier;
pub mod dart;
pub mod docker;
pub mod dotnet;
pub mod envless;
pub mod golang;
pub mod haskell;
pub mod julia;
pub mod lua;
pub mod node;
pub mod perl;
pub mod python;
pub mod r;
pub mod ruby;
pub mod rust;
pub mod swift;

use snp_core::lang::LanguageId;

use crate::Language;

/// The registry behind [`crate::backend`]: every `LanguageId` maps to its zero-sized backend.
pub(crate) fn registry(id: LanguageId) -> &'static dyn Language {
    match id {
        LanguageId::Conda => &conda::Conda,
        LanguageId::Coursier => &coursier::Coursier,
        LanguageId::Dart => &dart::Dart,
        LanguageId::Docker => &docker::Docker,
        LanguageId::DockerImage => &envless::DockerImage,
        LanguageId::Dotnet => &dotnet::Dotnet,
        LanguageId::Fail => &envless::Fail,
        LanguageId::Golang => &golang::Golang,
        LanguageId::Haskell => &haskell::Haskell,
        LanguageId::Julia => &julia::Julia,
        LanguageId::Lua => &lua::Lua,
        LanguageId::Node => &node::Node,
        LanguageId::Perl => &perl::Perl,
        LanguageId::Pygrep => &envless::Pygrep,
        LanguageId::Python => &python::Python,
        LanguageId::R => &r::R,
        LanguageId::Ruby => &ruby::Ruby,
        LanguageId::Rust => &rust::Rust,
        LanguageId::Swift => &swift::Swift,
        LanguageId::Unsupported => &envless::Unsupported,
        LanguageId::UnsupportedScript => &envless::UnsupportedScript,
    }
}
