//! `hookscripts::classify` on every era fixture of pre-commit's template plus prek, lefthook, snp 1.x, snp current,
//! a dangling symlink and a directory; at most 1 MiB is read (11 §A.5.1).

use std::path::{Path, PathBuf};

use snp_git::hookscripts::{Owner, SNP_SCRIPT_ID, classify};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/hookscripts")
        .join(name)
}

#[test]
fn classify_every_fixture() {
    for (name, want) in [
        ("pre-commit-era3", Owner::PreCommit { era: Some(3) }),
        ("pre-commit-era3-windows", Owner::PreCommit { era: Some(3) }),
        ("pre-commit-era2", Owner::PreCommit { era: Some(2) }),
        ("pre-commit-era1", Owner::PreCommit { era: Some(1) }),
        (
            "pre-commit-prior-4d9958c90bc262f47553e2c073f14cfe",
            Owner::PreCommit { era: None },
        ),
        (
            "pre-commit-prior-d8ee923c46731b42cd95cc869add4062",
            Owner::PreCommit { era: None },
        ),
        (
            "pre-commit-prior-49fd668cb42069aa1b6048464be5d395",
            Owner::PreCommit { era: None },
        ),
        (
            "pre-commit-prior-79f09a650522a87b0da915d0d983b2de",
            Owner::PreCommit { era: None },
        ),
        (
            "pre-commit-prior-e358c9dae00eac5d06b38dfdb1e33a8c",
            Owner::PreCommit { era: None },
        ),
        ("prek", Owner::Prek),
        ("lefthook", Owner::Lefthook),
        ("snp-v1", Owner::SnpV1),
        ("snp-current", Owner::Snp { stale: false }),
    ] {
        assert_eq!(classify(&fixture(name)).unwrap(), want, "{name}");
    }
    assert_eq!(
        classify(&fixture("pre-commit-era3")).unwrap().owner_name(),
        Some("pre-commit")
    );
}

#[test]
fn classify_special_files() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(classify(&d.path().join("missing")).unwrap(), Owner::Missing);
    assert_eq!(classify(d.path()).unwrap(), Owner::NotAFile);
    #[cfg(unix)]
    {
        let dangling = d.path().join("dangling");
        std::os::unix::fs::symlink(d.path().join("nowhere"), &dangling).unwrap();
        assert_eq!(classify(&dangling).unwrap(), Owner::Foreign);
        let to_script = d.path().join("link");
        std::os::unix::fs::symlink(fixture("prek"), &to_script).unwrap();
        assert_eq!(classify(&to_script).unwrap(), Owner::Prek);
        let to_dir = d.path().join("dirlink");
        std::os::unix::fs::symlink(d.path(), &to_dir).unwrap();
        assert_eq!(classify(&to_dir).unwrap(), Owner::NotAFile);
        let looped = d.path().join("loop");
        std::os::unix::fs::symlink(&looped, &looped).unwrap();
        assert_eq!(classify(&looped).unwrap(), Owner::Foreign);
        // a FIFO is never opened (it would block)
        let fifo = d.path().join("fifo");
        let st = std::process::Command::new("mkfifo").arg(&fifo).status();
        if st.is_ok_and(|s| s.success()) {
            assert_eq!(classify(&fifo).unwrap(), Owner::NotAFile);
        }
    }
}

#[test]
fn classify_reads_at_most_one_mib() {
    let d = tempfile::tempdir().unwrap();
    let mib = 1_usize << 20;
    let mut inside = vec![b'#'; mib - SNP_SCRIPT_ID.len()];
    inside.extend_from_slice(SNP_SCRIPT_ID.as_bytes());
    std::fs::write(d.path().join("inside"), &inside).unwrap();
    assert_eq!(
        classify(&d.path().join("inside")).unwrap(),
        Owner::Snp { stale: false }
    );
    let mut beyond = vec![b'#'; mib];
    beyond.extend_from_slice(SNP_SCRIPT_ID.as_bytes());
    std::fs::write(d.path().join("beyond"), &beyond).unwrap();
    assert_eq!(classify(&d.path().join("beyond")).unwrap(), Owner::Foreign);
}
