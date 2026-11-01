//! Deterministic corpora shared by the criterion micro-benches (ARCHITECTURE §12.3).
//!
//! The paths imitate a real repository listing (a few top-level trees, nested packages, tests, docs, assets and
//! dot-files, the extension mix of a polyglot project) without shipping one: a fixed linear congruential generator
//! picks the components, and every path is unique because its last component carries its index. The same corpus is
//! generated on every machine, so criterion baselines are comparable across runs.
//!
//! Owner: WP-36.

/// Top-level directories, weighted by repetition (the first ones are the large trees).
const TOP: &[&str] = &[
    "src",
    "src",
    "src",
    "lib",
    "tests",
    "tests",
    "docs",
    "pkg",
    "crates",
    "app",
    "scripts",
    "tools",
    "vendor",
    "node_modules",
    "migrations",
    "assets",
    "testing/resources",
    "pre_commit",
    ".github/workflows",
    "examples",
];

/// Inner directory names.
const MID: &[&str] = &[
    "core",
    "util",
    "api",
    "models",
    "views",
    "handlers",
    "internal",
    "fixtures",
    "components",
    "commands",
    "io",
    "parser",
    "net",
    "store",
    "config",
    "cli",
    "web",
    "db",
    "auth",
    "i18n",
    "data",
    "images",
    "templates",
    "v1",
];

/// File stems.
const STEM: &[&str] = &[
    "main", "mod", "lib", "index", "test_api", "conftest", "setup", "README", "utils", "helpers",
    "settings", "__init__", "schema", "client", "server", "worker", "types", "errors", "app",
    "router", "styles", "logo",
];

/// Extensions, weighted by repetition (Python, JavaScript/TypeScript and Rust dominate).
const EXT: &[&str] = &[
    "py", "py", "py", "py", "js", "js", "ts", "tsx", "jsx", "rs", "rs", "rs", "go", "md", "md",
    "rst", "txt", "yaml", "yml", "json", "json", "toml", "cfg", "ini", "html", "css", "min.js",
    "min.css", "png", "jpg", "svg", "sh", "pyi", "lock", "xml", "c", "h",
];

/// Non-ASCII directory and stem material for the Unicode corpus (Latin-1, Greek, Cyrillic, CJK, Devanagari,
/// Arabic digits, combining marks).
const UNICODE: &[&str] = &[
    "données",
    "Ελληνικά",
    "русский",
    "日本語",
    "中文资料",
    "हिन्दी",
    "٣٤٥",
    "e\u{301}cole",
    "Straße",
    "naïve",
    "Ωmega",
    "über",
    "café",
    "测试",
    "Ünïcödé",
];

/// The generator of Numerical Recipes (`x' = 1664525 x + 1013904223 mod 2^32`): deterministic, dependency-free.
#[derive(Debug, Clone)]
pub(crate) struct Lcg(u32);

impl Lcg {
    /// A generator with a fixed seed.
    pub(crate) const fn new(seed: u32) -> Self {
        Self(seed)
    }

    /// The next index below `n` (`n > 0`).
    pub(crate) fn below(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        // the high bits of an LCG are the random ones
        usize::try_from(self.0 >> 8).unwrap_or(0) % n
    }

    /// One element of `items` (non-empty).
    pub(crate) fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items.get(self.below(items.len())).copied().unwrap_or("x")
    }
}

/// `n` unique repository-relative paths (`/` separators), depth 1–5.
pub(crate) fn paths(n: usize) -> Vec<String> {
    corpus(n, 0x5eed_0001, false)
}

/// `n` unique paths whose directories and stems are non-ASCII (the ADR-079 B12 regex-cache case).
pub(crate) fn unicode_paths(n: usize) -> Vec<String> {
    corpus(n, 0x5eed_0002, true)
}

fn corpus(n: usize, seed: u32, unicode: bool) -> Vec<String> {
    let mut rng = Lcg::new(seed);
    let mut out = Vec::new();
    for i in 0..n {
        let mut p = String::new();
        if unicode {
            p.push_str(rng.pick(UNICODE));
        } else {
            p.push_str(rng.pick(TOP));
        }
        for _ in 0..rng.below(4) {
            p.push('/');
            p.push_str(if unicode && rng.below(2) == 0 {
                rng.pick(UNICODE)
            } else {
                rng.pick(MID)
            });
        }
        p.push('/');
        p.push_str(if unicode {
            rng.pick(UNICODE)
        } else {
            rng.pick(STEM)
        });
        p.push('_');
        p.push_str(&i.to_string());
        p.push('.');
        p.push_str(rng.pick(EXT));
        out.push(p);
    }
    out
}
