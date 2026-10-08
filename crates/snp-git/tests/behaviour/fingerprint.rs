//! `diff_fingerprint` equals blake3 of the real `git diff --no-ext-diff --no-textconv --ignore-submodules` bytes on
//! randomized trees (WBS WP-18 acceptance; `run._get_diff`, 02 §12).

use std::path::Path;

use snp_git::DiffFingerprint;

use crate::common::Sandbox;

/// xorshift64*: deterministic, dependency-free.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn bytes(&mut self, max: u64) -> Vec<u8> {
        let len = self.below(max);
        let binary = self.below(4) == 0;
        (0..len)
            .map(|_| {
                if binary {
                    u8::try_from(self.next() & 0xff).unwrap()
                } else {
                    b"abcdefghij \n\t\r"[usize::try_from(self.below(14)).unwrap()]
                }
            })
            .collect()
    }
}

fn real_diff(sb: &Sandbox, repo: &Path) -> Vec<u8> {
    sb.git(
        repo,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules",
        ],
    )
}

fn fingerprint_of(bytes: &[u8]) -> DiffFingerprint {
    DiffFingerprint {
        digest: *blake3::hash(bytes).as_bytes(),
        non_empty: !bytes.is_empty(),
    }
}

#[test]
fn diff_fingerprint_equals_blake3_of_real_git_diff() {
    assert_eq!(fingerprint_of(b""), DiffFingerprint::EMPTY);
    let sb = Sandbox::new();
    for seed in 1..=8_u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let repo = sb.init(&format!("r{seed}"));
        let n = 3 + rng.below(25);
        let names: Vec<String> = (0..n)
            .map(|i| match rng.below(4) {
                0 => format!("dir{}/f{i}.txt", rng.below(3)),
                1 => format!("sp ace {i}"),
                2 => format!("ü{i}.bin"),
                _ => format!("f{i}"),
            })
            .collect();
        for name in &names {
            let content = rng.bytes(400);
            sb.write(&repo, name, &content);
        }
        sb.git(&repo, &["add", "-A"]);
        sb.commit(&repo, "base");
        let (git, _) = sb.discover(&repo);
        assert_eq!(git.diff_fingerprint().unwrap(), DiffFingerprint::EMPTY);
        for _round in 0..4 {
            for name in &names {
                let p = repo.join(name);
                match rng.below(6) {
                    0 => {
                        let content = rng.bytes(400);
                        std::fs::write(&p, content).unwrap();
                    }
                    1 if p.exists() => {
                        let mut c = std::fs::read(&p).unwrap();
                        c.extend_from_slice(b"\nappended\n");
                        std::fs::write(&p, c).unwrap();
                    }
                    2 if p.exists() && rng.below(3) == 0 => std::fs::remove_file(&p).unwrap(),
                    #[cfg(unix)]
                    3 if p.exists() => {
                        use std::os::unix::fs::PermissionsExt;
                        let mode = if rng.below(2) == 0 { 0o755 } else { 0o644 };
                        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode))
                            .unwrap();
                    }
                    _ => {}
                }
            }
            // untracked files never count
            sb.write(&repo, &format!("untracked{}", rng.below(100)), b"x");
            let expected = fingerprint_of(&real_diff(&sb, &repo));
            assert_eq!(git.diff_fingerprint().unwrap(), expected, "seed {seed}");
        }
    }
}

#[test]
fn diff_fingerprint_honours_user_diff_config_like_pre_commit() {
    // the bytes change with user config (compared for equality only), exactly as git prints them
    let sb = Sandbox::new();
    let repo = sb.init("r");
    sb.write(&repo, "a", b"1\n2\n3\n");
    sb.git(&repo, &["add", "a"]);
    sb.commit(&repo, "base");
    sb.write(&repo, "a", b"1\nX\n3\n");
    sb.git(&repo, &["config", "diff.noprefix", "true"]);
    sb.git(&repo, &["config", "color.ui", "always"]);
    let (git, _) = sb.discover(&repo);
    let real = real_diff(&sb, &repo);
    assert!(
        memchr::memmem::find(&real, b"\x1b[").is_some(),
        "color.ui=always reaches the diff"
    );
    assert_eq!(git.diff_fingerprint().unwrap(), fingerprint_of(&real));
}
