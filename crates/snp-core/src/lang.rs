//! pre-commit's languages (`all_languages.py:27-50`; 01 §1.4).

/// One of the 21 languages of pre-commit 4.6.2, in `language_names` (sorted) order.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum LanguageId {
    /// `conda`
    Conda,
    /// `coursier`
    Coursier,
    /// `dart`
    Dart,
    /// `docker`
    Docker,
    /// `docker_image`
    DockerImage,
    /// `dotnet`
    Dotnet,
    /// `fail`
    Fail,
    /// `golang`
    Golang,
    /// `haskell`
    Haskell,
    /// `julia`
    Julia,
    /// `lua`
    Lua,
    /// `node`
    Node,
    /// `perl`
    Perl,
    /// `pygrep`
    Pygrep,
    /// `python`
    Python,
    /// `r`
    R,
    /// `ruby`
    Ruby,
    /// `rust`
    Rust,
    /// `swift`
    Swift,
    /// `unsupported` (alias `system`)
    Unsupported,
    /// `unsupported_script` (alias `script`)
    UnsupportedScript,
}

impl LanguageId {
    /// pre-commit's `language_names` (sorted, 01 §1.4).
    pub const ALL: [Self; 21] = [
        Self::Conda,
        Self::Coursier,
        Self::Dart,
        Self::Docker,
        Self::DockerImage,
        Self::Dotnet,
        Self::Fail,
        Self::Golang,
        Self::Haskell,
        Self::Julia,
        Self::Lua,
        Self::Node,
        Self::Perl,
        Self::Pygrep,
        Self::Python,
        Self::R,
        Self::Ruby,
        Self::Rust,
        Self::Swift,
        Self::Unsupported,
        Self::UnsupportedScript,
    ];

    /// The canonical config name, e.g. `docker_image`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Conda => "conda",
            Self::Coursier => "coursier",
            Self::Dart => "dart",
            Self::Docker => "docker",
            Self::DockerImage => "docker_image",
            Self::Dotnet => "dotnet",
            Self::Fail => "fail",
            Self::Golang => "golang",
            Self::Haskell => "haskell",
            Self::Julia => "julia",
            Self::Lua => "lua",
            Self::Node => "node",
            Self::Perl => "perl",
            Self::Pygrep => "pygrep",
            Self::Python => "python",
            Self::R => "r",
            Self::Ruby => "ruby",
            Self::Rust => "rust",
            Self::Swift => "swift",
            Self::Unsupported => "unsupported",
            Self::UnsupportedScript => "unsupported_script",
        }
    }

    /// A `language:` value: the canonical names plus `clientlib._translate_language`'s aliases
    /// `system` → `unsupported` and `script` → `unsupported_script` (`python_venv` was removed in
    /// 4.0.0 and is rejected).
    pub fn from_config(s: &str) -> Option<Self> {
        match s {
            "system" => Some(Self::Unsupported),
            "script" => Some(Self::UnsupportedScript),
            _ => Self::ALL.into_iter().find(|l| l.as_str() == s),
        }
    }

    /// `ENVIRONMENT_DIR` of the language module (`py_env`, `node_env`, …); `None` for the
    /// env-less languages `docker_image`, `fail`, `pygrep`, `unsupported`, `unsupported_script`.
    pub const fn env_dir_base(self) -> Option<&'static str> {
        match self {
            Self::Conda => Some("conda"),
            Self::Coursier => Some("coursier"),
            Self::Dart => Some("dartenv"),
            Self::Docker => Some("docker"),
            Self::Dotnet => Some("dotnetenv"),
            Self::Golang => Some("golangenv"),
            Self::Haskell => Some("hs_env"),
            Self::Julia => Some("juliaenv"),
            Self::Lua => Some("lua_env"),
            Self::Node => Some("node_env"),
            Self::Perl => Some("perl_env"),
            Self::Python => Some("py_env"),
            Self::R => Some("renv"),
            Self::Ruby => Some("rbenv"),
            Self::Rust => Some("rustenv"),
            Self::Swift => Some("swift_env"),
            Self::DockerImage
            | Self::Fail
            | Self::Pygrep
            | Self::Unsupported
            | Self::UnsupportedScript => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_equals_language_names() {
        let names: Vec<&str> = LanguageId::ALL.iter().map(|l| l.as_str()).collect();
        assert_eq!(
            names,
            [
                "conda",
                "coursier",
                "dart",
                "docker",
                "docker_image",
                "dotnet",
                "fail",
                "golang",
                "haskell",
                "julia",
                "lua",
                "node",
                "perl",
                "pygrep",
                "python",
                "r",
                "ruby",
                "rust",
                "swift",
                "unsupported",
                "unsupported_script"
            ]
        );
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, names, "language_names is sorted");
        let mut ord = LanguageId::ALL;
        ord.sort_unstable();
        assert_eq!(ord, LanguageId::ALL);
    }

    #[test]
    fn config_names_and_aliases() {
        for l in LanguageId::ALL {
            assert_eq!(LanguageId::from_config(l.as_str()), Some(l));
        }
        assert_eq!(
            LanguageId::from_config("system"),
            Some(LanguageId::Unsupported)
        );
        assert_eq!(
            LanguageId::from_config("script"),
            Some(LanguageId::UnsupportedScript)
        );
        assert_eq!(LanguageId::from_config("python_venv"), None);
        assert_eq!(LanguageId::from_config("Python"), None);
    }

    #[test]
    fn env_dirs() {
        let envless: Vec<LanguageId> = LanguageId::ALL
            .into_iter()
            .filter(|l| l.env_dir_base().is_none())
            .collect();
        assert_eq!(
            envless,
            [
                LanguageId::DockerImage,
                LanguageId::Fail,
                LanguageId::Pygrep,
                LanguageId::Unsupported,
                LanguageId::UnsupportedScript
            ]
        );
        assert_eq!(LanguageId::Python.env_dir_base(), Some("py_env"));
        assert_eq!(LanguageId::Haskell.env_dir_base(), Some("hs_env"));
    }
}
