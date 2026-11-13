//! `cargo xtask check-dag`: enforce the crate DAG of ARCHITECTURE §1.2 (normal, build and dev edges; xtask edges).
//!
//! Rules (§1.2, ADR-071 item 7, ADR-081):
//! * the workspace members are exactly the nodes of [`DAG`];
//! * a crate's normal dependencies on workspace crates and on the user crates (`cfgv`, `file-identify`) are exactly
//!   its listed edges, plus `snp-core` for every crate (`xtask` included) except `snp-pycompat`, `snp-probe` and
//!   `snp-core` itself;
//! * build dependencies are a subset of the normal edges;
//! * a crate may dev-depend on any of its ancestors, and on `snp-testkit` unless it is `snp-testkit` or one of its
//!   ancestors (no dev-cycles); `snp` may dev-depend on every crate;
//! * nothing depends (in any kind) on `snp` or `snp-difftest`.
//!
//! Owner: WP-00.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;

use anyhow::{Context as _, bail};

use crate::workspace;

/// The edge list of ARCHITECTURE §1.2 (without the implicit `snp-core` edge).
const DAG: &[(&str, &[&str])] = &[
    ("snp-pycompat", &[]),
    ("snp-core", &["snp-pycompat"]),
    ("snp-argparse", &["snp-pycompat"]),
    ("snp-cli", &["snp-argparse"]),
    ("snp-pyre", &["snp-pycompat"]),
    ("snp-yaml", &["snp-pycompat", "cfgv"]),
    ("snp-os", &["snp-pycompat"]),
    ("snp-proc", &["snp-pycompat", "snp-os", "file-identify"]),
    ("snp-net", &["snp-os"]),
    (
        "snp-lang",
        &["snp-pycompat", "snp-os", "snp-proc", "snp-net"],
    ),
    ("snp-pyparsers", &["snp-pycompat"]),
    ("snp-hooks-data", &[]),
    ("snp-git", &["snp-pycompat", "snp-os", "snp-proc"]),
    ("snp-testkit", &["snp-proc", "snp-git"]),
    ("snp-probe", &[]),
    (
        "snp-config",
        &[
            "snp-pycompat",
            "snp-yaml",
            "snp-pyre",
            "cfgv",
            "file-identify",
        ],
    ),
    ("snp-store", &["snp-pycompat", "snp-os"]),
    (
        "snp-files",
        &[
            "snp-pycompat",
            "snp-pyre",
            "snp-os",
            "snp-proc",
            "file-identify",
        ],
    ),
    (
        "snp-stash",
        &["snp-pycompat", "snp-os", "snp-proc", "snp-git"],
    ),
    (
        "snp-hooks",
        &[
            "snp-pycompat",
            "snp-pyre",
            "snp-argparse",
            "snp-cli",
            "snp-proc",
            "snp-os",
            "snp-git",
            "snp-pyparsers",
            "snp-hooks-data",
            "file-identify",
            "cfgv",
        ],
    ),
    (
        "snp-repo",
        &[
            "snp-config",
            "snp-store",
            "snp-git",
            "snp-lang",
            "snp-hooks-data",
            "snp-files",
            "snp-proc",
            "snp-os",
            "snp-pyre",
            "snp-net",
        ],
    ),
    (
        "snp-run",
        &[
            "snp-repo",
            "snp-stash",
            "snp-hooks",
            "snp-files",
            "snp-config",
            "snp-store",
            "snp-git",
            "snp-lang",
            "snp-proc",
            "snp-os",
            "snp-pyre",
            "snp-yaml",
            "snp-cli",
            "snp-pycompat",
            "snp-net",
        ],
    ),
    (
        "snp-cmd",
        &[
            "snp-config",
            "snp-store",
            "snp-git",
            "snp-stash",
            "snp-yaml",
            "snp-os",
            "snp-proc",
            "snp-cli",
            "snp-pycompat",
            "snp-hooks-data",
            "snp-pyre",
        ],
    ),
    (
        "snp-migrate",
        &[
            "snp-config",
            "snp-yaml",
            "snp-pyre",
            "snp-hooks-data",
            "file-identify",
        ],
    ),
    (
        "snp",
        &[
            "snp-run",
            "snp-cmd",
            "snp-migrate",
            "snp-repo",
            "snp-hooks",
            "snp-cli",
            "snp-store",
            "snp-proc",
            "snp-os",
            "snp-net",
            "snp-git",
            "snp-pyre",
            "snp-lang",
        ],
    ),
    ("snp-difftest", &["snp-testkit", "snp-yaml"]),
    ("xtask", &["snp-cli", "snp-hooks", "snp-argparse"]),
];

/// The sibling user crates: tracked as DAG leaves, not workspace members.
const USER_CRATES: [&str; 2] = ["cfgv", "file-identify"];
/// Crates without the implicit `snp-core` edge.
const NO_CORE: [&str; 3] = ["snp-pycompat", "snp-probe", "snp-core"];
/// Crates nothing may depend on.
const NEVER_DEPENDED_ON: [&str; 2] = ["snp", "snp-difftest"];

/// One crate's tracked dependencies by kind.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Deps {
    pub(crate) normal: BTreeSet<String>,
    pub(crate) build: BTreeSet<String>,
    pub(crate) dev: BTreeSet<String>,
}

/// Workspace members → their dependencies on tracked crates (members and user crates).
pub(crate) type Graph = BTreeMap<String, Deps>;

/// The allowed normal edges of every node (incl. the implicit `snp-core` edge).
fn allowed() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    DAG.iter()
        .map(|(name, deps)| {
            let mut set: BTreeSet<&str> = deps.iter().copied().collect();
            if !NO_CORE.contains(name) {
                set.insert("snp-core");
            }
            (*name, set)
        })
        .collect()
}

/// The transitive closure of the normal edges from `name` (excluding `name`).
fn ancestors(allowed: &BTreeMap<&str, BTreeSet<&str>>, name: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<&str> = allowed
        .get(name)
        .map_or_default(|s| s.iter().copied().collect());
    while let Some(n) = stack.pop() {
        if seen.insert(n.to_owned())
            && let Some(next) = allowed.get(n)
        {
            stack.extend(next.iter().copied());
        }
    }
    seen
}

/// Every violation of the §1.2 rules in `graph` (empty = clean).
pub(crate) fn check(graph: &Graph) -> Vec<String> {
    let allowed = allowed();
    let mut v = Vec::new();
    for name in allowed.keys() {
        if !graph.contains_key(*name) {
            v.push(format!(
                "crate `{name}` of the §1.2 DAG is not a workspace member"
            ));
        }
    }
    let testkit_ancestors = ancestors(&allowed, "snp-testkit");
    for (name, deps) in graph {
        let Some(edges) = allowed.get(name.as_str()) else {
            v.push(format!(
                "workspace member `{name}` is not in the §1.2 DAG (adding a crate needs an ADR)"
            ));
            continue;
        };
        for kind_deps in [&deps.normal, &deps.build, &deps.dev] {
            for d in kind_deps {
                if NEVER_DEPENDED_ON.contains(&d.as_str()) {
                    v.push(format!(
                        "`{name}` depends on `{d}`: nothing may depend on snp or snp-difftest"
                    ));
                }
            }
        }
        for d in &deps.normal {
            if !edges.contains(d.as_str()) && !NEVER_DEPENDED_ON.contains(&d.as_str()) {
                v.push(format!(
                    "forbidden edge `{name}` -> `{d}` (not in ARCHITECTURE §1.2)"
                ));
            }
        }
        for e in edges {
            if !deps.normal.contains(*e) {
                v.push(format!(
                    "missing edge `{name}` -> `{e}` (listed in ARCHITECTURE §1.2)"
                ));
            }
        }
        for d in &deps.build {
            if !edges.contains(d.as_str()) && !NEVER_DEPENDED_ON.contains(&d.as_str()) {
                v.push(format!(
                    "forbidden build edge `{name}` -> `{d}` (build deps must be §1.2 edges)"
                ));
            }
        }
        let own_ancestors = ancestors(&allowed, name);
        for d in &deps.dev {
            if NEVER_DEPENDED_ON.contains(&d.as_str()) {
                continue;
            }
            let ok = if name == "snp" {
                d != "snp"
            } else if d == "snp-testkit" {
                name != "snp-testkit" && !testkit_ancestors.contains(name)
            } else {
                own_ancestors.contains(d)
            };
            if !ok {
                let why = if d == "snp-testkit" {
                    "a dev-cycle: snp-testkit depends on this crate"
                } else {
                    "dev-dependencies must be ancestors (ARCHITECTURE §1.2)"
                };
                v.push(format!("forbidden dev edge `{name}` -> `{d}` ({why})"));
            }
        }
    }
    v
}

/// The tracked graph of the workspace from `cargo metadata --no-deps`.
pub(crate) fn graph_from_metadata(meta: &serde_json::Value) -> anyhow::Result<Graph> {
    let packages = meta
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .context("`cargo metadata` has no packages")?;
    let members: BTreeSet<&str> = packages
        .iter()
        .filter_map(|p| p.get("name").and_then(serde_json::Value::as_str))
        .collect();
    let mut graph = Graph::new();
    for p in packages {
        let name = p
            .get("name")
            .and_then(serde_json::Value::as_str)
            .context("package without a name")?;
        let mut deps = Deps::default();
        for d in p
            .get("dependencies")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(dep) = d.get("name").and_then(serde_json::Value::as_str) else {
                continue;
            };
            if !members.contains(dep) && !USER_CRATES.contains(&dep) {
                continue; // a third-party crate: not part of the DAG
            }
            let set = match d.get("kind").and_then(serde_json::Value::as_str) {
                None | Some("normal") => &mut deps.normal,
                Some("build") => &mut deps.build,
                Some("dev") => &mut deps.dev,
                Some(other) => bail!("`{name}`: unknown dependency kind `{other}`"),
            };
            set.insert(dep.to_owned());
        }
        graph.insert(name.to_owned(), deps);
    }
    Ok(graph)
}

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    if let Some(a) = args.first() {
        bail!("check-dag takes no arguments (got {})", a.to_string_lossy());
    }
    let root = workspace::root()?;
    let graph = graph_from_metadata(&workspace::metadata(&root)?)?;
    let violations = check(&graph);
    if violations.is_empty() {
        let edges: usize = graph.values().map(|d| d.normal.len()).sum();
        workspace::say(&format!(
            "check-dag: ok ({} crates, {edges} normal edges, ARCHITECTURE §1.2)",
            graph.len()
        ));
        return Ok(());
    }
    for v in &violations {
        workspace::say(&format!("check-dag: {v}"));
    }
    bail!("{} DAG violation(s)", violations.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The graph the §1.2 list describes (what a correct workspace looks like).
    fn clean() -> Graph {
        allowed()
            .into_iter()
            .map(|(name, edges)| {
                let deps = Deps {
                    normal: edges.iter().map(|e| (*e).to_owned()).collect(),
                    ..Deps::default()
                };
                (name.to_owned(), deps)
            })
            .collect()
    }

    fn plant(g: &mut Graph, from: &str, kind: &str, to: &str) {
        let d = g.get_mut(from).unwrap();
        let set = match kind {
            "normal" => &mut d.normal,
            "build" => &mut d.build,
            _ => &mut d.dev,
        };
        set.insert(to.to_owned());
    }

    #[test]
    fn the_listed_dag_is_clean() {
        assert_eq!(check(&clean()), Vec::<String>::new());
    }

    #[test]
    fn the_real_workspace_is_clean() {
        let root = workspace::root().unwrap();
        let g = graph_from_metadata(&workspace::metadata(&root).unwrap()).unwrap();
        assert_eq!(check(&g), Vec::<String>::new());
    }

    #[test]
    fn planted_forbidden_edge_fails() {
        let mut g = clean();
        plant(&mut g, "snp-pyre", "normal", "snp-yaml");
        let v = check(&g);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("forbidden edge `snp-pyre` -> `snp-yaml`"));
    }

    #[test]
    fn planted_dev_cycle_fails() {
        let mut g = clean();
        plant(&mut g, "snp-proc", "dev", "snp-testkit");
        let v = check(&g);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("dev-cycle"));
        // snp-net is not an ancestor of snp-testkit: allowed.
        let mut g = clean();
        plant(&mut g, "snp-net", "dev", "snp-testkit");
        assert_eq!(check(&g), Vec::<String>::new());
    }

    #[test]
    fn planted_xtask_edge_outside_cli_hooks_argparse_fails() {
        let mut g = clean();
        plant(&mut g, "xtask", "normal", "snp-run");
        let v = check(&g);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("`xtask` -> `snp-run`"));
        let mut g = clean();
        plant(&mut g, "xtask", "normal", "snp");
        assert!(
            check(&g)
                .iter()
                .any(|m| m.contains("nothing may depend on snp"))
        );
    }

    #[test]
    fn other_violations() {
        let mut g = clean();
        plant(&mut g, "snp", "dev", "snp-difftest");
        assert_eq!(check(&g).len(), 1);
        let mut g = clean();
        plant(&mut g, "snp-yaml", "dev", "snp-pyre");
        assert!(check(&g)[0].contains("must be ancestors"));
        let mut g = clean();
        plant(&mut g, "snp-git", "build", "snp-yaml");
        assert_eq!(check(&g).len(), 1);
        let mut g = clean();
        g.get_mut("snp-cli").unwrap().normal.remove("snp-argparse");
        assert!(check(&g)[0].contains("missing edge `snp-cli` -> `snp-argparse`"));
        let mut g = clean();
        g.insert("snp-new".to_owned(), Deps::default());
        assert!(check(&g)[0].contains("not in the §1.2 DAG"));
        // Allowed: a dev edge to an ancestor, `snp` dev-depending on anything.
        let mut g = clean();
        plant(&mut g, "snp-run", "dev", "snp-pycompat");
        plant(&mut g, "snp-run", "dev", "snp-testkit");
        plant(&mut g, "snp", "dev", "snp-yaml");
        assert_eq!(check(&g), Vec::<String>::new());
    }
}
