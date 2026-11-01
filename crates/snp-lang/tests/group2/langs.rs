//! Per-language rules of the group-2 backends (04b §2; 10 §5.10 rows for conda, coursier, dart, dotnet, haskell,
//! julia, lua, perl, r, swift).

use std::ffi::OsStr;
use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;

use snp_core::brand::Brand;
use snp_core::failure::Failure;
use snp_core::lang::LanguageId;
use snp_lang::harness::Reply;
use snp_lang::langs::conda::conda_exe;
use snp_lang::langs::coursier::missing_cs_message;
use snp_lang::langs::dart::{
    Executables, MAX_PUBSPEC, pubspec_executables, read_pubspec, valid_executable_name,
};
use snp_lang::langs::dotnet::{MAX_NUSPEC, NUGET_CONFIG, nuspec_tool_id, read_nuspec};
use snp_lang::langs::lua::lua_version;
use snp_lang::langs::r::{
    ENTRY_INDEX, ENTRY_NOT_RSCRIPT, ENTRY_SYNTAX, ENTRY_TWO_EXPRESSIONS, r_string_literal,
    rscript_exec,
};
use snp_lang::{Health, InstallState, RunPlan, VersionSupport, backend};

use super::util::{Fx, call_argvs, s, xargs_argv};

/// The `language_version` of every group-2 language is `default`.
#[test]
fn lang_group2_default_versions() {
    let fx = Fx::new(&[]);
    let host = fx.t.host();
    for id in GROUP2 {
        assert_eq!(&*backend(id).default_version(&host).unwrap(), "default");
    }
    assert!(fx.t.runner.calls().is_empty());
}

/// The eleven languages.
pub(crate) const GROUP2: [LanguageId; 11] = [
    LanguageId::Conda,
    LanguageId::Coursier,
    LanguageId::Dart,
    LanguageId::Docker,
    LanguageId::Dotnet,
    LanguageId::Haskell,
    LanguageId::Julia,
    LanguageId::Lua,
    LanguageId::Perl,
    LanguageId::R,
    LanguageId::Swift,
];

#[test]
fn lang_group2_basic_health_is_healthy_without_spawning() {
    let fx = Fx::new(&[]);
    for id in GROUP2.into_iter().filter(|id| *id != LanguageId::R) {
        assert_eq!(fx.health(id, None).unwrap(), Health::Healthy);
    }
    assert!(fx.t.runner.calls().is_empty());
}

// ---- conda ----

#[test]
fn lang_conda_exe_selection() {
    // conda_test.py:13-44 (non-empty means set, even "0").
    let env = |xs: &[(&str, &str)]| snp_core::env::Env::from_pairs(super::util::pairs(xs));
    assert_eq!(conda_exe(&env(&[])), "conda");
    assert_eq!(
        conda_exe(&env(&[
            ("PRE_COMMIT_USE_MICROMAMBA", "1"),
            ("PRE_COMMIT_USE_MAMBA", "")
        ])),
        "micromamba"
    );
    assert_eq!(
        conda_exe(&env(&[
            ("PRE_COMMIT_USE_MICROMAMBA", ""),
            ("PRE_COMMIT_USE_MAMBA", "1")
        ])),
        "mamba"
    );
    assert_eq!(conda_exe(&env(&[("PRE_COMMIT_USE_MAMBA", "0")])), "mamba");
    assert_eq!(
        conda_exe(&env(&[
            ("PRE_COMMIT_USE_MICROMAMBA", "x"),
            ("PRE_COMMIT_USE_MAMBA", "1")
        ])),
        "micromamba"
    );
}

#[test]
fn lang_conda_install_uses_the_selected_exe_without_deps_call() {
    let fx = Fx::new(&[("PRE_COMMIT_USE_MAMBA", "1")]);
    let out = fx.install(LanguageId::Conda, &[]).unwrap();
    assert_eq!(&*out.installer, "mamba");
    let env_dir = s(&fx.env_dir(LanguageId::Conda, "default"));
    assert_eq!(
        call_argvs(&fx.t),
        vec![vec![
            "mamba".to_owned(),
            "env".into(),
            "create".into(),
            "-p".into(),
            env_dir,
            "--file".into(),
            "environment.yml".into()
        ]]
    );
}

#[test]
fn lang_conda_activation_unsets_python_vars() {
    let fx = Fx::new(&[
        ("VIRTUAL_ENV", "/v"),
        ("PYTHONHOME", "/p"),
        ("CONDA_PREFIX", "/c"),
    ]);
    let env = fx.activated(LanguageId::Conda).unwrap();
    let env_dir = fx.env_dir(LanguageId::Conda, "default");
    assert_eq!(env.get("VIRTUAL_ENV"), None);
    assert_eq!(env.get("PYTHONHOME"), None);
    assert_eq!(env.get("CONDA_PREFIX"), Some(env_dir.as_os_str()));
    let path = env.get("PATH").unwrap().to_string_lossy().into_owned();
    if cfg!(windows) {
        let d = s(&env_dir);
        assert!(
            path.starts_with(&format!("{d}\\Library\\bin;{d}\\Scripts;{d};{d}\\bin;")),
            "{path}"
        );
    } else {
        assert!(
            path.starts_with(&format!("{}/bin:", env_dir.display())),
            "{path}"
        );
    }
}

// ---- coursier ----

#[test]
fn lang_coursier_requires_channel_or_deps() {
    let fx = Fx::new(&[]);
    fx.tool("cs");
    let e = fx.install(LanguageId::Coursier, &[]).unwrap_err();
    assert_eq!(
        e.to_string(),
        "expected .pre-commit-channel dir or additional_dependencies"
    );
    let f = Failure::from(e);
    assert_eq!(f.exit_code(), 1);
}

#[test]
fn lang_coursier_missing_cs_in_both_brands() {
    let mut fx = Fx::new(&[]);
    for brand in [Brand::PreCommit, Brand::Snp] {
        fx.t.brand = brand;
        let e = fx.install(LanguageId::Coursier, &["x"]).unwrap_err();
        assert_eq!(e.py_type(), "AssertionError");
        assert_eq!(e.to_string(), missing_cs_message(brand));
        assert_eq!(
            e.to_string(),
            format!(
                "{} requires system-installed \"cs\" or \"coursier\" executables in the application search path",
                brand.prog()
            )
        );
    }
}

#[test]
fn lang_coursier_falls_back_to_coursier_and_sorts_channel_apps() {
    let fx = Fx::new(&[]);
    fx.tool("coursier");
    fx.put(".pre-commit-channel/zeta.json", b"{}");
    fx.put(".pre-commit-channel/echo-java.json", b"{}");
    fx.put(".pre-commit-channel/a.b.json", b"{}");
    fx.put(".pre-commit-channel/.hidden", b"{}");
    let out = fx.install(LanguageId::Coursier, &[]).unwrap();
    assert_eq!(&*out.installer, "coursier");
    let apps: Vec<String> = call_argvs(&fx.t)
        .into_iter()
        .filter(|c| c[1] == "fetch")
        .map(|c| c.last().unwrap().clone())
        .collect();
    assert_eq!(apps, [".hidden", "a.b", "echo-java", "zeta"]);
    let calls = call_argvs(&fx.t);
    assert_eq!(calls[0][0], "coursier");
    assert_eq!(
        calls[0][1..4],
        ["fetch", "--default-channels=false", "--channel"]
    );
}

#[test]
fn lang_coursier_empty_channel_dir_creates_the_env() {
    // pre-commit: FileNotFoundError on its state file; snp: an empty environment (04b §5.10 #8).
    let fx = Fx::new(&[]);
    fx.tool("cs");
    std::fs::create_dir_all(fx.prefix.join(".pre-commit-channel")).unwrap();
    fx.install(LanguageId::Coursier, &[]).unwrap();
    assert!(fx.t.runner.calls().is_empty());
    assert!(fx.env_dir(LanguageId::Coursier, "default").is_dir());
}

// ---- dart ----

#[test]
fn lang_dart_pubspec_shapes() {
    let names = |xs: &[&str]| Executables::Names(xs.iter().map(|x| (*x).to_owned()).collect());
    let then = |xs: &[&str], ty| {
        Executables::NamesThenNonStr(xs.iter().map(|x| (*x).to_owned()).collect(), ty)
    };
    let cases: Vec<(&str, Executables)> = vec![
        ("name: x\nexecutables: {}\n", names(&[])),
        ("name: x\n", Executables::Missing),
        ("# nothing\n", Executables::EmptyDocument),
        ("name: x\nexecutables:\n", Executables::Null),
        ("executables: null\n", Executables::Null),
        ("executables: ~ # none\n", Executables::Null),
        ("executables: 5\n", Executables::NotIterable("int")),
        ("executables: yes\n", Executables::NotIterable("bool")),
        (
            "name: x\nexecutables:\n  a:\n  b: other\n  'c d':\n    # nested\nversion: 1\n",
            names(&["a", "b", "c d"]),
        ),
        (
            "executables: {a: b, c, \"d\": e}\n",
            names(&["a", "c", "d"]),
        ),
        ("executables: [a, 'b']\n", names(&["a", "b"])),
        ("executables:\n- a\n- b\nname: x\n", names(&["a", "b"])),
        ("executables:\n  - a\n", names(&["a"])),
        ("---\nexecutables: {b, a: }\n...\n", names(&["b", "a"])),
        ("\u{feff}executables:\n  x: y # c\n", names(&["x"])),
        ("executables: {a}\nexecutables:\n  b:\n", names(&["b"])),
        (
            "environment:\n  sdk: '>=2.12.0'\nexecutables:\n  \"tab\\tname\":\n",
            names(&["tab\tname"]),
        ),
        // A string is iterated by characters; an empty one installs nothing.
        ("executables: abc\n", names(&["a", "b", "c"])),
        ("executables: ''\n", names(&[])),
        // A flow collection on its own line is the value, not a key (review finding).
        ("executables:\n  {a: b}\n", names(&["a"])),
        ("executables:\n  {a: b, c: d}\n", names(&["a", "c"])),
        ("executables:\n  [a, b]\n", names(&["a", "b"])),
        // Duplicate keys collapse into the first position, like a dict.
        ("executables:\n  a:\n  b:\n  a: c\n", names(&["a", "b"])),
        ("executables: {a: b, a: c}\n", names(&["a"])),
        // Keys PyYAML resolves to another type: the loop crashes there (after the names before it).
        ("executables:\n  a:\n  123:\n  b:\n", then(&["a"], "int")),
        ("executables:\n  yes:\n", then(&[], "bool")),
        ("executables:\n  null:\n", then(&[], "NoneType")),
        ("executables:\n  ~:\n", then(&[], "NoneType")),
        ("executables:\n  1.5:\n", then(&[], "float")),
        ("executables:\n  0x1f:\n", then(&[], "int")),
        ("executables:\n  1:30:\n", then(&[], "int")),
        (
            "executables:\n  '123':\n  yess:\n  2to3x:\n",
            names(&["123", "yess", "2to3x"]),
        ),
        ("executables:\n  - a\n  - {b: c}\n", then(&["a"], "dict")),
        // Multi-line plain scalars and flow collections over several lines.
        (
            "description: a\n  b\nexecutables: [a,\n  b]\n",
            names(&["a", "b"]),
        ),
        ("executables: {\n  a: b,\n}\n", names(&["a"])),
        ("executables: {a: [b]}\n", names(&["a"])),
    ];
    for (text, want) in cases {
        assert_eq!(pubspec_executables(text).unwrap(), want, "{text:?}");
    }
    // Invalid YAML (PyYAML raises) and constructs outside the subset.
    for bad in [
        "- a\n",
        "executables: &a {}\n",
        "executables:\n  ? a\n",
        "a: 1\n---\nb: 2\n",
        "  indented: 1\n",
        "executables:\n\t- a\n",
        "executables: [a\n",
        "executables: [a, b]\n- a\n",
        "executables:\n  x: y: z\n",
        "executables:\n  foo: *x\n",
        "executables:\n  : x\n",
        "executables:\n  @foo:\n",
        "executables:\n  %foo:\n",
        "executables:\n odd:\nname: x\n  \"d\":\n",
        "description: |\n  foo\n# comment\n  bar\nexecutables:\n  x:\n",
        "executables:\n  0b_:\n",
        "x: =\nexecutables:\n  a:\n",
        "executables:\n  <<: {a: b}\n",
        "...\n",
        "x: \u{1}\nexecutables:\n  a:\n",
    ] {
        assert!(pubspec_executables(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn lang_dart_pubspec_reader_is_total() {
    // Deterministic pseudo-random inputs from the YAML alphabet: no panic, every result a value or an error.
    let alphabet = b"ab:-{}[], '\"\\#\n\t ~&*?|>%.x\x00\xc3\xa9";
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        usize::try_from(state % 4096).unwrap()
    };
    for _ in 0..20_000 {
        let mut text = Vec::new();
        for _ in 0..next() % 64 {
            text.push(alphabet[next() % alphabet.len()]);
        }
        let text = String::from_utf8_lossy(&text);
        drop(pubspec_executables(&format!("executables:{text}")));
        drop(pubspec_executables(&text));
    }
}

#[test]
fn lang_dart_executable_names_are_single_components() {
    for ok in ["tool", "my_tool", ".hidden", "a.b", "-x"] {
        assert!(valid_executable_name(ok), "{ok}");
    }
    for bad in ["", ".", "..", "../x", "a/b", "/abs", "nul\0"] {
        assert!(!valid_executable_name(bad), "{bad:?}");
    }
    assert_eq!(valid_executable_name("a\\b"), !cfg!(windows));
}

#[test]
fn lang_dart_rejects_a_traversing_executable_name() {
    let fx = Fx::new(&[]);
    fx.put("pubspec.yaml", b"name: x\nexecutables:\n  ../../evil:\n");
    let e = fx.install(LanguageId::Dart, &[]).unwrap_err();
    assert_eq!(e.py_type(), "ValueError");
    assert!(e.to_string().contains("'../../evil'"), "{e}");
    // `dart pub get` ran (pre-commit reads the key after it), no compile.
    let calls = call_argvs(&fx.t);
    assert_eq!(calls, vec![vec!["dart", "pub", "get"]]);
}

#[test]
fn lang_dart_missing_or_null_executables_fail() {
    for (body, frag) in [
        ("name: x\n", "no `executables` key"),
        (
            "name: x\nexecutables:\n",
            "'NoneType' object is not iterable",
        ),
        ("name: x\nexecutables: 1\n", "'int' object is not iterable"),
        ("# empty\n", "'NoneType' object is not subscriptable"),
    ] {
        let fx = Fx::new(&[]);
        fx.put("pubspec.yaml", body.as_bytes());
        let e = fx.install(LanguageId::Dart, &[]).unwrap_err();
        assert!(e.to_string().contains(frag), "{body}: {e}");
        assert_eq!(Failure::from(e).exit_code(), 3, "{body}");
        // pre-commit crashes after `dart pub get`.
        assert_eq!(
            call_argvs(&fx.t),
            vec![vec!["dart", "pub", "get"]],
            "{body}"
        );
    }
}

#[test]
fn lang_dart_non_str_key_crashes_after_the_names_before_it() {
    let fx = Fx::new(&[]);
    fx.put(
        "pubspec.yaml",
        b"name: x\nexecutables:\n  a:\n  123:\n  b:\n",
    );
    let e = fx.install(LanguageId::Dart, &[]).unwrap_err();
    assert_eq!(
        e.to_string(),
        "join() argument must be str, bytes, or os.PathLike object, not 'int'"
    );
    assert_eq!(Failure::from(e).exit_code(), 3);
    let calls = call_argvs(&fx.t);
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls[1][5].ends_with("a.dart"), "{calls:?}");
}

#[test]
fn lang_dart_duplicate_keys_compile_once() {
    let fx = Fx::new(&[]);
    fx.put("pubspec.yaml", b"name: x\nexecutables:\n  a:\n  a: b\n");
    fx.install(LanguageId::Dart, &[]).unwrap();
    let calls = call_argvs(&fx.t);
    assert_eq!(calls.len(), 2, "{calls:?}");
}

#[test]
fn lang_dart_invalid_pubspec_fails_before_pub_get() {
    let fx = Fx::new(&[]);
    fx.put("pubspec.yaml", b"name: x\nexecutables: [a, b]\n- a\n");
    let e = fx.install(LanguageId::Dart, &[]).unwrap_err();
    assert_eq!(Failure::from(e).exit_code(), 3);
    assert!(fx.t.runner.calls().is_empty());
}

#[test]
fn lang_dart_pubspec_cap_refuses_instead_of_truncating() {
    // A file of exactly the cap is read; one byte more is refused (never parsed in part).
    let cap = usize::try_from(MAX_PUBSPEC).unwrap();
    let head = b"name: x\nexecutables:\n  a:\n";
    let mut body = head.to_vec();
    body.resize(cap, b'\n');
    let fx = Fx::new(&[]);
    fx.put("pubspec.yaml", &body);
    assert_eq!(
        read_pubspec(&fx.prefix).unwrap(),
        Executables::Names(vec!["a".to_owned()])
    );
    body.push(b'\n');
    fx.put("pubspec.yaml", &body);
    let e = read_pubspec(&fx.prefix).unwrap_err();
    assert_eq!(e.py_type(), "ValueError");
    assert!(
        e.to_string()
            .ends_with("pubspec.yaml is larger than 1048576 bytes"),
        "{e}"
    );
    let e = fx.install(LanguageId::Dart, &[]).unwrap_err();
    assert!(e.to_string().contains("larger than"), "{e}");
    assert!(fx.t.runner.calls().is_empty());
}

#[test]
fn lang_dart_bin_dir_must_not_exist_and_dep_without_pubspec_fails() {
    let fx = Fx::new(&[]);
    let bin = fx.env_dir(LanguageId::Dart, "default").join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let e = fx.install(LanguageId::Dart, &[]).unwrap_err();
    assert_eq!(e.py_type(), "FileExistsError");
    let fx = Fx::new(&[]);
    let e = fx.install(LanguageId::Dart, &["nope:1.0"]).unwrap_err();
    assert_eq!(e.to_string(), "could not find pubspec.yaml for nope:1.0");
    assert_eq!(e.py_type(), "AssertionError");
}

#[test]
fn lang_dart_compiles_windows_exe_names() {
    let fx = Fx::new(&[]);
    fx.put("pubspec.yaml", b"name: x\nexecutables:\n  t:\n");
    fx.install(LanguageId::Dart, &[]).unwrap();
    let calls = call_argvs(&fx.t);
    let out = &calls[1][4];
    assert_eq!(out.ends_with("t.exe"), cfg!(windows), "{out}");
    assert!(calls[1][5].ends_with("t.dart"));
}

// ---- dotnet ----

/// A zip at `path` with the given members.
fn zip_with(path: &Path, members: &[(&str, &[u8])]) {
    let f = std::fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    for (name, bytes) in members {
        z.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(bytes).unwrap();
    }
    z.finish().unwrap();
}

#[test]
fn lang_dotnet_nuspec_parsing() {
    let ns = "http://schemas.microsoft.com/packaging/2013/05/nuspec.xsd";
    let doc =
        |body: &str| format!("<?xml version=\"1.0\"?><package xmlns=\"{ns}\">{body}</package>");
    assert_eq!(
        nuspec_tool_id(&doc("<metadata><id>testeroni.tool</id></metadata>")).unwrap(),
        "testeroni.tool"
    );
    // The first `id` in document order.
    assert_eq!(
        nuspec_tool_id(&doc(
            "<a><id>first</id></a><metadata><id>second</id></metadata>"
        ))
        .unwrap(),
        "first"
    );
    for (xml, text) in [
        (
            "<package><metadata><id>x</id></metadata></package>".to_owned(),
            "could not parse namespace from nuspec",
        ),
        (doc("<metadata/>"), "expected to find an \"id\" element"),
        (
            doc("<metadata><id></id></metadata>"),
            "\"id\" element missing tool name",
        ),
        (
            doc("<metadata><id/></metadata>"),
            "\"id\" element missing tool name",
        ),
        // An `id` in another namespace does not count.
        (
            doc("<metadata><id xmlns=\"urn:other\">x</id></metadata>"),
            "expected to find an \"id\" element",
        ),
    ] {
        let e = nuspec_tool_id(&xml).unwrap_err();
        assert_eq!(e.to_string(), text, "{xml}");
        assert_eq!(e.py_type(), "AssertionError");
    }
    assert_eq!(
        nuspec_tool_id("<not xml").unwrap_err().py_type(),
        "ValueError"
    );
}

/// The fuzzing invariant (256 KiB thread stacks): deeply nested elements are read without recursion (roxmltree
/// >= 0.21.2 parses nested elements iteratively; 0.21.1 overflowed on the fuzz smoke's input).
#[test]
fn lang_dotnet_nuspec_deep_nesting_fits_a_small_stack() {
    let ns = "http://schemas.microsoft.com/packaging/2013/05/nuspec.xsd";
    let deep = format!(
        "<package xmlns=\"{ns}\">{}<id>x</id>{}</package>",
        "<a>".repeat(5000),
        "</a>".repeat(5000)
    );
    let unclosed = "<C>".repeat(5000);
    let worker = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            (
                nuspec_tool_id(&deep).ok(),
                nuspec_tool_id(&unclosed).is_err(),
            )
        })
        .unwrap();
    assert_eq!(worker.join().unwrap(), (Some("x".to_owned()), true));
}

#[test]
fn lang_dotnet_nuspec_entry_rules_and_cap() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("x.nupkg");
    zip_with(&p, &[("a.txt", b"x")]);
    assert_eq!(
        read_nuspec(&p).unwrap_err().to_string(),
        "not enough values to unpack (expected 1, got 0)"
    );
    zip_with(&p, &[("a.nuspec", b"<x/>"), ("b.nuspec", b"<y/>")]);
    assert_eq!(
        read_nuspec(&p).unwrap_err().to_string(),
        "too many values to unpack (expected 1)"
    );
    zip_with(&p, &[("a.nuspec", b"\xef\xbb\xbf<x/>")]);
    assert_eq!(read_nuspec(&p).unwrap(), "<x/>");
    // A nuspec larger than 1 MiB (here a highly compressible one) is refused.
    let big = vec![b' '; usize::try_from(MAX_NUSPEC).unwrap() + 1];
    zip_with(&p, &[("a.nuspec", &big)]);
    let e = read_nuspec(&p).unwrap_err();
    assert!(e.to_string().contains("larger than 1048576 bytes"), "{e}");
    std::fs::write(&p, b"not a zip").unwrap();
    assert_eq!(read_nuspec(&p).unwrap_err().py_type(), "ValueError");
}

#[test]
fn lang_dotnet_no_outputs_and_sorted_packages() {
    let fx = Fx::new(&[]);
    std::fs::create_dir_all(fx.prefix.join("pre-commit-build")).unwrap();
    let e = fx.install(LanguageId::Dotnet, &[]).unwrap_err();
    assert_eq!(e.to_string(), "could not find any build outputs to install");
    let fx = Fx::new(&[]);
    let build = fx.prefix.join("pre-commit-build");
    std::fs::create_dir_all(&build).unwrap();
    let ns = "http://schemas.microsoft.com/packaging/2013/05/nuspec.xsd";
    for id in ["zz.tool", "aa.tool"] {
        let xml = format!("<package xmlns=\"{ns}\"><metadata><id>{id}</id></metadata></package>");
        zip_with(
            &build.join(format!("{id}.1.0.0.nupkg")),
            &[(&format!("{id}.nuspec"), xml.as_bytes())],
        );
    }
    std::fs::write(build.join("ignored.txt"), b"").unwrap();
    fx.install(LanguageId::Dotnet, &[]).unwrap();
    let ids: Vec<String> = call_argvs(&fx.t)
        .into_iter()
        .filter(|c| c[1] == "tool")
        .map(|c| c.last().unwrap().clone())
        .collect();
    assert_eq!(ids, ["aa.tool", "zz.tool"]);
    assert_eq!(
        NUGET_CONFIG,
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><configuration>  <packageSources>    <clear />  \
         </packageSources></configuration>"
    );
}

// ---- haskell ----

#[test]
fn lang_haskell_requires_cabal_files_or_deps() {
    let fx = Fx::new(&[]);
    let e = fx.install(LanguageId::Haskell, &[]).unwrap_err();
    assert_eq!(
        e.to_string(),
        "Expected .cabal files or additional_dependencies"
    );
    assert_eq!(Failure::from(e).exit_code(), 1);
    assert!(fx.t.runner.calls().is_empty());
    let fx = Fx::new(&[]);
    fx.put("z.cabal", b"");
    fx.put("a.cabal", b"");
    fx.put(".cabal", b"");
    fx.install(LanguageId::Haskell, &["dep"]).unwrap();
    let calls = call_argvs(&fx.t);
    assert_eq!(calls[1][6..], [".cabal", "a.cabal", "z.cabal", "dep"]);
    assert!(
        fx.env_dir(LanguageId::Haskell, "default")
            .join("bin")
            .is_dir()
    );
}

// ---- julia ----

#[test]
fn lang_julia_ignores_language_version() {
    assert_eq!(
        backend(LanguageId::Julia).support().version,
        VersionSupport::Ignored
    );
    assert!(backend(LanguageId::Julia).support().deps);
    let fx = Fx::new(&[]);
    fx.install_v(LanguageId::Julia, "1.9.3", &[]).unwrap();
    let env_dir = fx.env_dir(LanguageId::Julia, "1.9.3");
    assert!(env_dir.join("Project.toml").is_file());
    let calls = call_argvs(&fx.t);
    assert_eq!(calls[0][..3], ["julia", "--startup-file=no", "-e"]);
    assert_eq!(calls[0][4..], ["--".to_owned(), s(&env_dir)]);
    assert!(snp_lang::check_support(LanguageId::Julia, "1.9.3", &[], Brand::Snp).is_ok());
}

#[test]
fn lang_julia_copies_project_files_with_priority() {
    let fx = Fx::new(&[]);
    fx.put("Project.toml", b"p");
    fx.put("JuliaProject.toml", b"jp");
    fx.put("Manifest.toml", b"m");
    fx.install(LanguageId::Julia, &["Example@0.5"]).unwrap();
    let env_dir = fx.env_dir(LanguageId::Julia, "default");
    assert_eq!(
        std::fs::read(env_dir.join("JuliaProject.toml")).unwrap(),
        b"jp"
    );
    assert!(!env_dir.join("Project.toml").exists());
    assert_eq!(std::fs::read(env_dir.join("Manifest.toml")).unwrap(), b"m");
    assert_eq!(call_argvs(&fx.t)[0].last().unwrap(), "Example@0.5");
}

#[test]
fn lang_julia_script_is_joined_unless_local() {
    let fx = Fx::new(&[]);
    let env = fx.t.env.clone();
    let remote = fx
        .plan(LanguageId::Julia, "bin/hook.jl --a", &["b"], false, &env)
        .unwrap();
    let joined = fx.prefix.join("bin").join("hook.jl");
    assert_eq!(
        xargs_argv(&remote),
        [
            "julia".to_owned(),
            "--startup-file=no".into(),
            s(&joined),
            "--a".into(),
            "b".into()
        ]
    );
    let local = fx
        .plan(LanguageId::Julia, "bin/hook.jl", &[], true, &env)
        .unwrap();
    assert_eq!(
        xargs_argv(&local),
        ["julia", "--startup-file=no", "bin/hook.jl"]
    );
    let empty = fx.plan(LanguageId::Julia, "", &[], true, &env).unwrap();
    assert!(matches!(empty, RunPlan::Error { code: 1, .. }));
}

#[test]
fn lang_julia_activation() {
    let fx = Fx::new(&[("JULIA_PROJECT", "@.")]);
    let env = fx.activated(LanguageId::Julia).unwrap();
    let d = fx.env_dir(LanguageId::Julia, "default");
    assert_eq!(env.get("JULIA_PROJECT"), None);
    assert_eq!(env.get("JULIA_LOAD_PATH"), Some(d.as_os_str()));
    let sep = if cfg!(windows) { ";" } else { ":" };
    assert_eq!(
        env.get("JULIA_DEPOT_PATH").unwrap().to_string_lossy(),
        format!("{}{sep}", d.join("depot").display())
    );
}

// ---- lua ----

#[test]
fn lang_lua_version_probed_once_per_run() {
    let fx = Fx::new(&[]);
    fx.t.runner
        .reply("luarocks", Reply::ok(" 5.3 \n").when(["config"]).sticky());
    fx.install(LanguageId::Lua, &["a"]).unwrap();
    for _ in 0..3 {
        fx.activated(LanguageId::Lua).unwrap();
    }
    let host = fx.t.host();
    assert_eq!(&*lua_version(&host).unwrap(), "5.3");
    let probes = call_argvs(&fx.t)
        .into_iter()
        .filter(|c| c[1] == "config")
        .count();
    assert_eq!(probes, 1);
    // The memo lives in the run's ToolProbe, not in the process: a fresh probe (another run) asks again, and the Lua
    // language_version is still `default`.
    assert_eq!(
        &*backend(LanguageId::Lua).default_version(&host).unwrap(),
        "default"
    );
    let fresh = snp_lang::ToolProbe::new(None);
    let other = snp_lang::Host {
        probe: &fresh,
        ..fx.t.host()
    };
    assert_eq!(&*lua_version(&other).unwrap(), "5.3");
    let probes = call_argvs(&fx.t)
        .into_iter()
        .filter(|c| c[1] == "config")
        .count();
    assert_eq!(probes, 2);
    let env = fx.activated(LanguageId::Lua).unwrap();
    let d = fx.env_dir(LanguageId::Lua, "default");
    let lua_path = env.get("LUA_PATH").unwrap().to_string_lossy().into_owned();
    let sep = std::path::MAIN_SEPARATOR;
    assert_eq!(
        lua_path,
        format!(
            "{d}{sep}share{sep}lua{sep}5.3{sep}?.lua;{d}{sep}share{sep}lua{sep}5.3{sep}?{sep}init.lua;;",
            d = d.display()
        )
    );
    let so = if cfg!(windows) { "dll" } else { "so" };
    assert_eq!(
        env.get("LUA_CPATH").unwrap().to_string_lossy(),
        format!("{}{sep}lib{sep}lua{sep}5.3{sep}?.{so};;", d.display())
    );
}

#[test]
fn lang_lua_failing_probe_is_called_process_error_and_not_memoised() {
    let fx = Fx::new(&[]);
    fx.t.runner
        .reply("luarocks", Reply::fail(1, "").when(["config"]));
    let e = fx.activated(LanguageId::Lua).unwrap_err();
    assert_eq!(e.py_type(), "CalledProcessError");
    // The next activation probes again (and now succeeds with the default empty output).
    fx.activated(LanguageId::Lua).unwrap();
    assert_eq!(call_argvs(&fx.t).len(), 2);
}

#[test]
fn lang_lua_non_utf8_version_is_lossy() {
    // D-CRASH-13.
    let fx = Fx::new(&[]);
    fx.t.runner.reply(
        "luarocks",
        Reply::ok(b"5.4\xff\n".to_vec()).when(["config"]),
    );
    let host = fx.t.host();
    assert_eq!(&*lua_version(&host).unwrap(), "5.4\u{fffd}");
}

#[test]
fn lang_lua_rockspecs_sorted() {
    let fx = Fx::new(&[]);
    fx.put("b.rockspec", b"");
    fx.put("a.rockspec", b"");
    fx.install(LanguageId::Lua, &[]).unwrap();
    let made: Vec<String> = call_argvs(&fx.t)
        .into_iter()
        .filter(|c| c.get(3).is_some_and(|a| a == "make"))
        .map(|c| c[4].clone())
        .collect();
    assert_eq!(
        made,
        [
            "a.rockspec",
            "b.rockspec",
            "pre-commit-package-dev-1.rockspec"
        ]
    );
}

// ---- perl ----

#[test]
fn lang_perl_quotes_the_install_base() {
    let fx = Fx::new(&[("PERL5LIB", "/old")]);
    let env = fx.activated(LanguageId::Perl).unwrap();
    let d = fx.env_dir(LanguageId::Perl, "default");
    let q = snp_pycompat::shlex::quote(&s(&d)).into_owned();
    assert_eq!(
        env.get("PERL5LIB"),
        Some(d.join("lib").join("perl5").as_os_str())
    );
    assert_eq!(
        env.get("PERL_MB_OPT").unwrap().to_string_lossy(),
        format!("--install_base {q}")
    );
    assert_eq!(
        env.get("PERL_MM_OPT").unwrap().to_string_lossy(),
        format!("INSTALL_BASE={q} INSTALLSITEMAN1DIR=none INSTALLSITEMAN3DIR=none")
    );
    assert_eq!(snp_pycompat::shlex::quote("/a b/it's"), "'/a b/it'\"'\"'s'");
}

#[test]
fn lang_perl_install_creates_the_env_dir() {
    let fx = Fx::new(&[]);
    fx.install(LanguageId::Perl, &[]).unwrap();
    assert_eq!(call_argvs(&fx.t), vec![vec!["cpan", "-T", "."]]);
    assert!(fx.env_dir(LanguageId::Perl, "default").is_dir());
}

// ---- r ----

#[test]
fn lang_r_entry_validation_texts() {
    let fx = Fx::new(&[]);
    let env = fx.t.env.clone();
    for (entry, text) in [
        ("Rscript -e 1 2", ENTRY_TWO_EXPRESSIONS),
        ("Rscript a b", ENTRY_SYNTAX),
        ("Rscript", ENTRY_INDEX),
        ("rscript -e 1", ENTRY_NOT_RSCRIPT),
    ] {
        let e = fx.plan(LanguageId::R, entry, &[], false, &env).unwrap_err();
        assert_eq!(e.to_string(), text, "{entry}");
        assert_eq!(Failure::from(e).exit_code(), 3);
    }
    assert_eq!(ENTRY_NOT_RSCRIPT, "entry must start with `Rscript`.");
    assert_eq!(
        ENTRY_TWO_EXPRESSIONS,
        "You can supply at most one expression."
    );
    assert_eq!(
        ENTRY_SYNTAX,
        "The only valid syntax is `Rscript -e {expr}`or `Rscript path/to/hook/script`"
    );
    for (entry, text) in [
        ("", "Executable `` not found"),
        ("Rscript 'x", "ValueError: No closing quotation"),
    ] {
        let p = fx.plan(LanguageId::R, entry, &[], false, &env).unwrap();
        let RunPlan::Error { output, .. } = p else {
            panic!("{p:?}")
        };
        assert_eq!(output, text.as_bytes());
    }
}

#[test]
fn lang_r_command_forms() {
    // r_test.py:19-130.
    let fx = Fx::new(&[]);
    let env = fx.t.env.clone();
    let opts = [
        "--no-save",
        "--no-restore",
        "--no-site-file",
        "--no-environ",
    ];
    let expr = fx
        .plan(LanguageId::R, "Rscript -e 'cat(1)'", &["x"], false, &env)
        .unwrap();
    let mut want: Vec<String> = vec!["Rscript".into()];
    want.extend(opts.iter().map(|o| (*o).to_owned()));
    want.extend(["-e".to_owned(), "cat(1)".into(), "x".into()]);
    assert_eq!(xargs_argv(&expr), want);
    let remote = fx
        .plan(LanguageId::R, "Rscript hooks/a.R", &[], false, &env)
        .unwrap();
    assert_eq!(
        xargs_argv(&remote).last().unwrap(),
        &s(&fx.prefix.join("hooks").join("a.R"))
    );
    let local = fx
        .plan(LanguageId::R, "Rscript hooks/a.R", &[], true, &env)
        .unwrap();
    assert_eq!(xargs_argv(&local).last().unwrap(), "hooks/a.R");
    // No hazmat rewrite for R.
    assert!(
        fx.plan(LanguageId::R, "pre-commit hazmat x", &[], true, &env)
            .is_err()
    );
}

#[test]
fn lang_r_rscript_exec() {
    let env = |xs: &[(&str, &str)]| snp_core::env::Env::from_pairs(super::util::pairs(xs));
    assert_eq!(rscript_exec(&env(&[])), "Rscript");
    let exe = if cfg!(windows) {
        "Rscript.exe"
    } else {
        "Rscript"
    };
    let sep = std::path::MAIN_SEPARATOR;
    assert_eq!(
        rscript_exec(&env(&[("R_HOME", "/opt/R")])).to_string_lossy(),
        format!("/opt/R{sep}bin{sep}{exe}")
    );
}

#[test]
fn lang_r_prefix_literal_matches_python_repr() {
    for p in [
        "/a/b",
        "/it's",
        "/both'\"",
        "/back\\slash",
        "/tab\there",
        "/é/ü",
        "/x\u{7f}",
    ] {
        assert_eq!(
            r_string_literal(OsStr::new(p)),
            snp_pycompat::repr::repr_str(p),
            "{p:?}"
        );
    }
    // Non-printable non-ASCII: R's \U{…} (Python's \x85 would be a raw byte in R).
    assert_eq!(
        r_string_literal(OsStr::new("/a\u{85}")),
        "'/a\\U{00000085}'"
    );
}

/// A native (non-`#!`) fake Rscript under `<dir>/bin`, outside any `shims` directory, for `R_HOME=<dir>`.
fn native_r_home(dir: &Path) -> std::path::PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let rscript = bin.join(if cfg!(windows) {
        "Rscript.exe"
    } else {
        "Rscript"
    });
    std::fs::write(&rscript, b"\x7fELF fake").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&rscript, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    rscript
}

/// The install state recorded from `out`.
fn r_state(out: &snp_lang::InstallOutcome) -> InstallState {
    InstallState {
        v: InstallState::VERSION,
        language: Arc::from("r"),
        language_version: Arc::from("default"),
        deps: Arc::from(Vec::new()),
        installer: Arc::clone(&out.installer),
        fingerprint: out.fingerprint.clone(),
        snp_version: Arc::from("x"),
    }
}

#[test]
fn lang_r_health_fast_path_and_texts() {
    let home = tempfile::tempdir().unwrap();
    let home_dir = snp_pycompat::ospath::realpath(home.path().as_os_str(), home.path()).unwrap();
    let rscript = native_r_home(&home_dir);
    let fx = Fx::new(&[("R_HOME", home_dir.to_str().unwrap())]);
    fx.t.runner.reply("Rscript", Reply::ok("4.3.3").sticky());
    let out = fx.install(LanguageId::R, &[]).unwrap();
    let fp = out.fingerprint.clone().unwrap();
    assert_eq!(fp.exe, rscript);
    let installs = fx.t.runner.calls().len();
    let state = r_state(&out);
    // Unchanged Rscript: healthy without spawning.
    assert_eq!(
        fx.health(LanguageId::R, Some(&state)).unwrap(),
        Health::Healthy
    );
    assert_eq!(fx.t.runner.calls().len(), installs);
    // A replaced Rscript (other size): pre-commit's two spawns.
    std::fs::write(&rscript, b"\x7fELF upgraded").unwrap();
    assert_eq!(
        fx.health(LanguageId::R, Some(&state)).unwrap(),
        Health::Healthy
    );
    assert_eq!(fx.t.runner.calls().len(), installs + 2);
}

#[test]
fn lang_r_health_never_trusts_a_shim_or_script_rscript() {
    // `Fx::tool` writes a `#!/bin/sh` script into a `shims` directory: an asdf/mise-style proxy whose file stays the
    // same while the R version behind it changes. No fingerprint, and health always runs pre-commit's two scripts.
    let fx = Fx::new(&[]);
    fx.tool("Rscript");
    fx.t.runner.reply("Rscript", Reply::ok("4.3.3").sticky());
    let out = fx.install(LanguageId::R, &[]).unwrap();
    assert_eq!(out.fingerprint, None);
    let installs = fx.t.runner.calls().len();
    let mut state = r_state(&out);
    assert_eq!(
        fx.health(LanguageId::R, Some(&state)).unwrap(),
        Health::Healthy
    );
    assert_eq!(fx.t.runner.calls().len(), installs + 2);
    // Even a state carrying a matching fingerprint of the shim (e.g. recorded by an older snp) is not trusted.
    state.fingerprint =
        fx.t.probe
            .fingerprint(&fx.shims.join("Rscript"))
            .map(|mut f| {
                f.detail = Arc::from("Rscript");
                f
            });
    assert!(state.fingerprint.is_some());
    fx.health(LanguageId::R, Some(&state)).unwrap();
    assert_eq!(fx.t.runner.calls().len(), installs + 4);
    // A native Rscript reached through a symlink in a `shims` directory is a proxy too.
    #[cfg(unix)]
    {
        let real = tempfile::tempdir().unwrap();
        let target = native_r_home(real.path());
        let fx = Fx::new(&[]);
        std::os::unix::fs::symlink(&target, fx.shims.join("Rscript")).unwrap();
        fx.t.runner.reply("Rscript", Reply::ok("4.3.3").sticky());
        assert_eq!(fx.install(LanguageId::R, &[]).unwrap().fingerprint, None);
    }
}

#[test]
fn lang_r_health_messages() {
    for (installed, text) in [
        (
            "NULL",
            "Hooks were installed with an unknown R version. R version for hook repo now set to 4.4.0",
        ),
        (
            "",
            "Hooks were installed with an unknown R version. R version for hook repo now set to 4.4.0",
        ),
        (
            "4.3.3",
            "Hooks were installed for R version 4.3.3, but current R executable has version 4.4.0",
        ),
    ] {
        let fx = Fx::new(&[]);
        fx.t.runner
            .reply("Rscript", Reply::ok(format!("{installed}\n")));
        fx.t.runner.reply("Rscript", Reply::ok("4.4.0\n"));
        assert_eq!(
            fx.health(LanguageId::R, None).unwrap(),
            Health::Unhealthy(text.to_owned())
        );
    }
    let fx = Fx::new(&[]);
    fx.t.runner.reply("Rscript", Reply::fail(2, "boom"));
    assert_eq!(
        fx.health(LanguageId::R, None).unwrap_err().py_type(),
        "CalledProcessError"
    );
}

#[test]
fn lang_r_install_copies_renv_and_requires_the_lock() {
    let fx = Fx::with_template(&[], false);
    let e = fx.install(LanguageId::R, &[]).unwrap_err();
    assert_eq!(e.py_type(), "FileNotFoundError");
    let fx = Fx::new(&[]);
    fx.install(LanguageId::R, &[]).unwrap();
    let d = fx.env_dir(LanguageId::R, "default");
    assert!(d.join("renv.lock").is_file());
    assert!(d.join("renv").join("activate.R").is_file());
    // The temporary scripts are gone.
    assert_eq!(std::fs::read_dir(&fx.tmp).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn lang_copytree_refuses_a_symlink_loop_quickly() {
    // `renv/x -> .` and `renv/y -> .`: depth-bounded recursion alone would make ~2^64 copies.
    let fx = Fx::new(&[]);
    let renv = fx.prefix.join("renv");
    std::os::unix::fs::symlink(".", renv.join("x")).unwrap();
    std::os::unix::fs::symlink(".", renv.join("y")).unwrap();
    let start = std::time::Instant::now();
    let e = fx.install(LanguageId::R, &[]).unwrap_err();
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
    assert!(e.to_string().contains("symlink loop"), "{e}");
    assert!(fx.t.runner.calls().is_empty());
    // A symlink to a sibling directory (no loop) is followed and copied, as by shutil.copytree.
    let fx = Fx::new(&[]);
    let renv = fx.prefix.join("renv");
    std::fs::create_dir_all(fx.prefix.join("other")).unwrap();
    std::fs::write(fx.prefix.join("other").join("f"), b"x").unwrap();
    std::os::unix::fs::symlink("../other", renv.join("link")).unwrap();
    fx.install(LanguageId::R, &[]).unwrap();
    let d = fx.env_dir(LanguageId::R, "default");
    assert_eq!(
        std::fs::read(d.join("renv").join("link").join("f")).unwrap(),
        b"x"
    );
}

// ---- swift ----

#[test]
fn lang_swift_builds_in_the_run_cwd() {
    let fx = Fx::new(&[]);
    fx.install(LanguageId::Swift, &[]).unwrap();
    let calls = fx.t.runner.calls();
    assert_eq!(calls[0].cwd, fx.repo);
    let d = fx.env_dir(LanguageId::Swift, "default");
    assert_eq!(
        call_argvs(&fx.t)[0],
        [
            "swift".to_owned(),
            "build".into(),
            "--package-path".into(),
            s(&fx.prefix),
            "-c".into(),
            "release".into(),
            "--build-path".into(),
            s(&d.join(".build")),
        ]
    );
    // `os.mkdir`: the environment must not exist.
    let e = fx.install(LanguageId::Swift, &[]).unwrap_err();
    assert_eq!(e.py_type(), "FileExistsError");
    let env = fx.activated(LanguageId::Swift).unwrap();
    assert!(
        env.get("PATH")
            .unwrap()
            .to_string_lossy()
            .starts_with(&s(&d.join(".build").join("release")))
    );
}

#[test]
fn lang_swift_rejects_deps_like_pre_commit() {
    let fx = Fx::new(&[]);
    let e = fx.install(LanguageId::Swift, &["x"]).unwrap_err();
    assert_eq!(
        e.to_string(),
        "for now, snp does not support additional_dependencies for swift -- you selected \
         `additional_dependencies: ['x']`"
    );
    assert!(fx.t.runner.calls().is_empty());
}
