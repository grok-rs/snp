//! `common::rmtree`: readonly-aware (`util_test.py:404-412`, Go's read-only module cache), contained, never follows
//! a symlink.

use std::io;
use std::path::Path;

use snp_lang::common;

#[cfg(unix)]
fn chmod(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn removes_a_tree_with_read_only_parts() {
    let d = tempfile::tempdir().unwrap();
    let env = d.path().join("golangenv-default");
    let pkg = env.join("pkg/mod/example.com/x@v1.0.0");
    std::fs::create_dir_all(pkg.join("sub")).unwrap();
    std::fs::write(pkg.join("go.mod"), b"module x\n").unwrap();
    std::fs::write(pkg.join("sub/a.go"), b"package sub\n").unwrap();
    #[cfg(unix)]
    {
        chmod(&pkg.join("go.mod"), 0o444);
        chmod(&pkg.join("sub/a.go"), 0o444);
        chmod(&pkg.join("sub"), 0o555);
        chmod(&pkg, 0o555);
        // A directory without any permission at all.
        std::fs::create_dir_all(env.join("pkg/locked")).unwrap();
        std::fs::write(env.join("pkg/locked/f"), b"f").unwrap();
        chmod(&env.join("pkg/locked"), 0o000);
    }
    #[cfg(not(unix))]
    {
        let mut p = std::fs::metadata(pkg.join("go.mod")).unwrap().permissions();
        p.set_readonly(true);
        std::fs::set_permissions(pkg.join("go.mod"), p).unwrap();
    }
    common::rmtree(&env.join("pkg"), &env).unwrap();
    assert!(!env.join("pkg").exists());
    assert!(env.exists());
}

#[cfg(unix)]
#[test]
fn symlinks_are_unlinked_never_followed() {
    let d = tempfile::tempdir().unwrap();
    let outside = d.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("precious"), b"keep").unwrap();
    let root = d.path().join("root");
    let tree = root.join("tree");
    std::fs::create_dir_all(&tree).unwrap();
    std::os::unix::fs::symlink(&outside, tree.join("link-to-dir")).unwrap();
    std::os::unix::fs::symlink(outside.join("precious"), tree.join("link-to-file")).unwrap();
    common::rmtree(&tree, &root).unwrap();
    assert!(!tree.exists());
    assert_eq!(std::fs::read(outside.join("precious")).unwrap(), b"keep");
    // The target itself being a link: the link goes, its target stays.
    std::os::unix::fs::symlink(&outside, root.join("l")).unwrap();
    common::rmtree(&root.join("l"), &root).unwrap();
    assert!(std::fs::symlink_metadata(root.join("l")).is_err());
    assert!(outside.join("precious").exists());
    // A path below a symlinked directory is refused rather than reached through the link.
    std::os::unix::fs::symlink(&outside, root.join("via")).unwrap();
    assert!(common::rmtree(&root.join("via/precious"), &root).is_err());
    assert!(outside.join("precious").exists());
}

#[test]
fn removal_is_contained_and_missing_paths_are_fine() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("envs");
    std::fs::create_dir_all(root.join("a")).unwrap();
    for bad in [
        root.clone(),
        d.path().to_path_buf(),
        root.join("../x"),
        d.path().join("elsewhere"),
    ] {
        let e = common::rmtree(&bad, &root).unwrap_err();
        assert_eq!(
            e.kind(),
            io::ErrorKind::PermissionDenied,
            "{}",
            bad.display()
        );
    }
    assert!(root.join("a").exists());
    common::rmtree(&root.join("missing"), &root).unwrap();
    common::rmtree(&root.join("missing/deeper"), &root).unwrap();
    // A file.
    std::fs::write(root.join("a/f"), b"x").unwrap();
    common::rmtree(&root.join("a/f"), &root).unwrap();
    assert!(!root.join("a/f").exists());
}

#[cfg(unix)]
#[test]
fn deep_trees_do_not_recurse() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let mut p = root.join("deep");
    for _ in 0..300 {
        p.push("d");
    }
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("leaf"), b"x").unwrap();
    common::rmtree(&root.join("deep"), root).unwrap();
    assert!(!root.join("deep").exists());
}
