//! One zero-sized backend per language, registered in `backend` (§6.2).
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
