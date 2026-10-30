//! The group-1 texts in both brands (WBS `DoD`: messages golden-tested in both brands): none names the tool, so the
//! pre-commit and the snp brand print the same bytes.

use snp_core::brand::Brand;
use snp_lang::Language;
use snp_lang::harness::Reply;
use snp_lang::langs::golang::Golang;
use snp_lang::langs::node::Node;
use snp_lang::langs::python::Python;

use crate::support::{FakeFetch, World, deps, env_ctx, install_ctx};

/// Every user-visible error text of the group-1 backends that a test can provoke hermetically.
fn texts(brand: Brand) -> Vec<String> {
    let mut out = Vec::new();
    let w = World::new();
    std::fs::remove_file(w.prefix.join("package.json")).unwrap();
    let no_deps = deps(&[]);
    for (installer, version) in [("conda", "python3.12"), ("pip", "python3.99")] {
        let mut t = w.host(w.isolated_env(&[("SNP_PYTHON_INSTALLER", installer)]));
        t.brand = brand;
        let host = t.host();
        let env_dir = w.prefix.join(format!("py_env-{installer}"));
        let e = Python
            .install(&install_ctx(
                &host, &w.prefix, version, &env_dir, &no_deps, false,
            ))
            .unwrap_err();
        out.push(e.to_string());
    }
    let mut t = w.host(w.isolated_env(&[("SNP_PYTHON_INSTALLER", "uv")]));
    t.brand = brand;
    let host = t.host();
    let env_dir = w.prefix.join("py_env-uv");
    let dash = deps(&["--user"]);
    let e = Python
        .install(&install_ctx(
            &host,
            &w.prefix,
            "python3.12",
            &env_dir,
            &dash,
            false,
        ))
        .unwrap_err();
    out.push(e.to_string());
    let env_dir = w.prefix.join("node_env-system");
    let e = Node
        .install(&install_ctx(
            &host, &w.prefix, "system", &env_dir, &no_deps, true,
        ))
        .unwrap_err();
    out.push(e.to_string());
    t.runner.reply("node", Reply::fail(127, ""));
    let h = Node
        .health(&env_ctx(&host, &w.prefix, "system", &env_dir), None)
        .unwrap();
    out.push(format!("{h:?}"));
    let mut t = w.host(w.isolated_env(&[]));
    t.brand = brand;
    t.fetch = Some(Box::new(FakeFetch {
        texts: vec![(
            "https://go.dev/dl/?mode=json&include=all".to_owned(),
            "[]".to_owned(),
        )],
        ..FakeFetch::default()
    }));
    let host = t.host();
    let env_dir = w.prefix.join("golangenv-9.9.9");
    let e = Golang
        .install(&install_ctx(
            &host, &w.prefix, "9.9.9", &env_dir, &no_deps, false,
        ))
        .unwrap_err();
    out.push(
        e.to_string()
            .split(" (os=")
            .next()
            .unwrap_or_default()
            .to_owned(),
    );
    out
}

#[test]
fn lang_group1_messages_in_both_brands() {
    let pre_commit = texts(Brand::PreCommit);
    assert_eq!(pre_commit, texts(Brand::Snp));
    assert_eq!(
        pre_commit,
        [
            "SNP_PYTHON_INSTALLER must be one of `auto`, `uv`, `pip` (got 'conda')",
            "failed to find interpreter for python_spec='python3.99'",
            "SNP_PYTHON_INSTALLER=uv: the additional dependency `--user` is a pip option uv does not accept; set \
             SNP_PYTHON_INSTALLER=auto or pip to build it with pip",
            "`language: node` must have package.json or additional_dependencies",
            "Unhealthy(\"`node --version` returned 127\")",
            "Could not find a version matching your system requirements",
        ]
    );
}
