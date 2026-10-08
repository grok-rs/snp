//! The task table: one file per task, each owned by the package that needs it (WBS "Disjoint ownership"). A task
//! is `fn(&[OsString]) -> anyhow::Result<()>` over the arguments that follow its name and parses them itself.
//!
//! Owner: WP-00 (this file); every task file names its owner.

use std::ffi::{OsStr, OsString};

mod bench;
mod catalog_index;
mod catalog_status;
mod check_dag;
mod check_unsafe;
mod cli_goldens;
mod completions;
mod difftest;
mod divergence_book;
mod docs;
mod gen_pch_data;
mod lint_guards;
mod oracle;
mod pch_gates;
mod pch_goldens;
mod platform_facts;
mod pycompat_vectors;
mod pyre_vectors;
mod release;
mod scenario_gen;
mod tool_checksums;
mod yaml_vectors;

/// One `cargo xtask <name>` command.
#[derive(Debug)]
pub(crate) struct Task {
    /// The command name (kebab-case).
    pub(crate) name: &'static str,
    /// The owning work package.
    pub(crate) owner: &'static str,
    /// The entry point.
    pub(crate) run: fn(&[OsString]) -> anyhow::Result<()>,
}

/// Every task, sorted by name.
pub(crate) const TASKS: &[Task] = &[
    Task {
        name: "bench",
        owner: "WP-36",
        run: bench::run,
    },
    Task {
        name: "catalog-index",
        owner: "WP-00",
        run: catalog_index::run,
    },
    Task {
        name: "catalog-status",
        owner: "WP-35",
        run: catalog_status::run,
    },
    Task {
        name: "check-dag",
        owner: "WP-00",
        run: check_dag::run,
    },
    Task {
        name: "check-unsafe",
        owner: "WP-00",
        run: check_unsafe::run,
    },
    Task {
        name: "cli-goldens",
        owner: "WP-12",
        run: cli_goldens::run,
    },
    Task {
        name: "completions",
        owner: "WP-12",
        run: completions::run,
    },
    Task {
        name: "difftest",
        owner: "WP-35",
        run: difftest::run,
    },
    Task {
        name: "divergence-book",
        owner: "WP-35",
        run: divergence_book::run,
    },
    Task {
        name: "docs",
        owner: "WP-37",
        run: docs::run,
    },
    Task {
        name: "gen-pch-data",
        owner: "WP-17",
        run: gen_pch_data::run,
    },
    Task {
        name: "lint-guards",
        owner: "WP-00",
        run: lint_guards::run,
    },
    Task {
        name: "oracle",
        owner: "WP-00",
        run: oracle::run,
    },
    Task {
        name: "pch-gates",
        owner: "WP-27",
        run: pch_gates::run,
    },
    Task {
        name: "pch-goldens",
        owner: "WP-26",
        run: pch_goldens::run,
    },
    Task {
        name: "platform-facts",
        owner: "WP-37",
        run: platform_facts::run,
    },
    Task {
        name: "pycompat-vectors",
        owner: "WP-11",
        run: pycompat_vectors::run,
    },
    Task {
        name: "pyre-vectors",
        owner: "WP-13",
        run: pyre_vectors::run,
    },
    Task {
        name: "release",
        owner: "WP-37",
        run: release::run,
    },
    Task {
        name: "scenario-gen",
        owner: "WP-35",
        run: scenario_gen::run,
    },
    Task {
        name: "tool-checksums",
        owner: "WP-16",
        run: tool_checksums::run,
    },
    Task {
        name: "yaml-vectors",
        owner: "WP-14",
        run: yaml_vectors::run,
    },
];

/// The task named `name`.
pub(crate) fn find(name: &OsStr) -> Option<&'static Task> {
    TASKS.iter().find(|t| name == t.name)
}
