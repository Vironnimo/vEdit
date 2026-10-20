//! One filename classification for labels, syntax, comments and language servers.
//! Extensions are case-insensitive; language IDs follow the LSP identifiers.
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Rust,
    Python,
    Dart,
    JavaScript,
    TypeScript,
    Json,
    Toml,
    Markdown,
    Html,
    Xml,
    Css,
    Scss,
    Less,
    C,
    Cpp,
    CSharp,
    Go,
    Java,
    Kotlin,
    Swift,
    Yaml,
    Shell,
    PowerShell,
    Batch,
    Ruby,
    Php,
    Lua,
    Sql,
    R,
    Julia,
    Ini,
    Dockerfile,
    Makefile,
    CMake,
    Latex,
    Ignore,
    PlainText,
}

pub fn from_path(path: Option<&Path>) -> FileKind {
    let Some(path) = path else {
        return FileKind::PlainText;
    };
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match name.as_str() {
        "dockerfile" | "containerfile" => return FileKind::Dockerfile,
        "makefile" | "gnumakefile" | "bsdmakefile" => return FileKind::Makefile,
        "cmakelists.txt" => return FileKind::CMake,
        ".bashrc" | ".bash_profile" | ".bash_login" | ".profile" | ".zshrc" | ".zprofile"
        | ".zshenv" => return FileKind::Shell,
        ".gitignore" | ".gitattributes" | ".ignore" => return FileKind::Ignore,
        ".gitmodules" | ".editorconfig" => return FileKind::Ini,
        ".env" => return FileKind::Shell,
        _ => {}
    }
    if name.starts_with("dockerfile.") || name.starts_with("containerfile.") {
        return FileKind::Dockerfile;
    }
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => FileKind::Rust,
        "py" | "pyi" | "pyw" => FileKind::Python,
        "dart" => FileKind::Dart,
        "js" | "jsx" | "mjs" | "cjs" => FileKind::JavaScript,
        "ts" | "tsx" | "mts" | "cts" => FileKind::TypeScript,
        "json" | "jsonc" | "json5" => FileKind::Json,
        "toml" => FileKind::Toml,
        "md" | "markdown" | "mdown" | "mkd" => FileKind::Markdown,
        "html" | "htm" => FileKind::Html,
        "xml" | "svg" | "xsl" | "xsd" => FileKind::Xml,
        "css" => FileKind::Css,
        "scss" | "sass" => FileKind::Scss,
        "less" => FileKind::Less,
        "c" => FileKind::C,
        "h" | "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => FileKind::Cpp,
        "cs" => FileKind::CSharp,
        "go" => FileKind::Go,
        "java" => FileKind::Java,
        "kt" | "kts" => FileKind::Kotlin,
        "swift" => FileKind::Swift,
        "yaml" | "yml" => FileKind::Yaml,
        "sh" | "bash" | "zsh" | "fish" => FileKind::Shell,
        "ps1" | "psm1" | "psd1" => FileKind::PowerShell,
        "bat" | "cmd" => FileKind::Batch,
        "rb" | "rake" | "gemspec" => FileKind::Ruby,
        "php" | "phtml" => FileKind::Php,
        "lua" => FileKind::Lua,
        "sql" => FileKind::Sql,
        "r" => FileKind::R,
        "jl" => FileKind::Julia,
        "ini" | "cfg" | "conf" => FileKind::Ini,
        "mk" | "mak" => FileKind::Makefile,
        "cmake" => FileKind::CMake,
        "tex" | "sty" | "cls" => FileKind::Latex,
        _ => FileKind::PlainText,
    }
}

pub fn language_id(path: Option<&Path>) -> &'static str {
    from_path(path).language_id()
}

pub fn comment_language(path: Option<&Path>) -> &str {
    if matches!(from_path(path), FileKind::PlainText | FileKind::Ini) {
        // Less common file types still use editor.rs's existing marker table.
        // INI-like files share highlighting, but `.conf` and `.cfg` comment
        // with `#` while `.ini` uses `;`.
        path.and_then(Path::extension)
            .and_then(|value| value.to_str())
            .unwrap_or("plaintext")
    } else {
        language_id(path)
    }
}

impl FileKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Python => "Python",
            Self::Dart => "Dart",
            Self::JavaScript => "JavaScript",
            Self::TypeScript => "TypeScript",
            Self::Json => "JSON",
            Self::Toml => "TOML",
            Self::Markdown => "Markdown",
            Self::Html => "HTML",
            Self::Xml => "XML",
            Self::Css => "CSS",
            Self::Scss => "SCSS",
            Self::Less => "Less",
            Self::C => "C",
            Self::Cpp => "C++",
            Self::CSharp => "C#",
            Self::Go => "Go",
            Self::Java => "Java",
            Self::Kotlin => "Kotlin",
            Self::Swift => "Swift",
            Self::Yaml => "YAML",
            Self::Shell => "Shell",
            Self::PowerShell => "PowerShell",
            Self::Batch => "Batch",
            Self::Ruby => "Ruby",
            Self::Php => "PHP",
            Self::Lua => "Lua",
            Self::Sql => "SQL",
            Self::R => "R",
            Self::Julia => "Julia",
            Self::Ini => "INI",
            Self::Dockerfile => "Dockerfile",
            Self::Makefile => "Makefile",
            Self::CMake => "CMake",
            Self::Latex => "LaTeX",
            Self::Ignore => "Ignore patterns",
            Self::PlainText => "Plain text",
        }
    }

    pub fn language_id(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::Dart => "dart",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Json => "json",
            Self::Toml => "toml",
            Self::Markdown => "markdown",
            Self::Html => "html",
            Self::Xml => "xml",
            Self::Css => "css",
            Self::Scss => "scss",
            Self::Less => "less",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::CSharp => "csharp",
            Self::Go => "go",
            Self::Java => "java",
            Self::Kotlin => "kotlin",
            Self::Swift => "swift",
            Self::Yaml => "yaml",
            Self::Shell => "shellscript",
            Self::PowerShell => "powershell",
            Self::Batch => "bat",
            Self::Ruby => "ruby",
            Self::Php => "php",
            Self::Lua => "lua",
            Self::Sql => "sql",
            Self::R => "r",
            Self::Julia => "julia",
            Self::Ini => "ini",
            Self::Dockerfile => "dockerfile",
            Self::Makefile => "makefile",
            Self::CMake => "cmake",
            Self::Latex => "latex",
            Self::Ignore => "ignore",
            Self::PlainText => "plaintext",
        }
    }

    pub fn syntax(self) -> &'static str {
        match self {
            Self::Rust => "rs",
            Self::Python => "py",
            Self::Dart => "dart",
            Self::JavaScript | Self::TypeScript => "js",
            Self::Json => "json",
            Self::Toml => "toml",
            Self::Markdown => "md",
            Self::Html => "html",
            Self::Xml => "xml",
            Self::Css | Self::Scss | Self::Less => "css",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::CSharp => "cs",
            Self::Go => "go",
            Self::Java => "java",
            Self::Kotlin => "kt",
            Self::Swift => "swift",
            Self::Yaml => "yaml",
            Self::Shell => "sh",
            Self::PowerShell => "ps1",
            Self::Batch => "bat",
            Self::Ruby => "rb",
            Self::Php => "php",
            Self::Lua => "lua",
            Self::Sql => "sql",
            Self::R => "r",
            Self::Julia => "jl",
            Self::Ini => "ini",
            Self::Dockerfile => "Dockerfile",
            Self::Makefile => "Makefile",
            Self::CMake => "cmake",
            Self::Latex => "tex",
            Self::Ignore | Self::PlainText => "txt",
        }
    }

    pub fn badge(self) -> &'static str {
        match self {
            Self::Rust => "rs",
            Self::Python => "Py",
            Self::Dart => "D",
            Self::JavaScript => "JS",
            Self::TypeScript => "TS",
            Self::Json => "{}",
            Self::Toml | Self::Yaml | Self::Ini => "≡",
            Self::Markdown => "MD",
            Self::Html | Self::Xml => "<>",
            Self::Css | Self::Scss | Self::Less => "#",
            Self::Shell | Self::PowerShell | Self::Batch => ">_",
            _ => "·",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_source_stub_and_gui_extensions_share_language_and_comments() {
        for name in ["main.py", "types.pyi", "window.pyw", "MAIN.PY", "TYPES.PYI"] {
            let kind = from_path(Some(Path::new(name)));
            assert_eq!(kind, FileKind::Python, "{name}");
            assert_eq!(kind.syntax(), "py");
            assert_eq!(language_id(Some(Path::new(name))), "python");
            assert_eq!(
                crate::editor::toggle_line_comment("value: int", 0..0, kind.label()).text,
                "# value: int"
            );
        }
    }

    #[test]
    fn named_files_and_case_insensitive_extensions_have_consistent_identity() {
        for (name, kind) in [
            ("Dockerfile", FileKind::Dockerfile),
            ("Dockerfile.dev", FileKind::Dockerfile),
            ("Containerfile", FileKind::Dockerfile),
            ("Makefile", FileKind::Makefile),
            ("CMakeLists.txt", FileKind::CMake),
            (".zshrc", FileKind::Shell),
            ("main.DART", FileKind::Dart),
            ("script.PS1", FileKind::PowerShell),
            ("README.MD", FileKind::Markdown),
            ("theme.CSS", FileKind::Css),
        ] {
            assert_eq!(from_path(Some(Path::new(name))), kind, "{name}");
        }
        for name in ["Dockerfile", "Makefile", "CMakeLists.txt", ".bashrc"] {
            let kind = from_path(Some(Path::new(name)));
            assert!(
                crate::editor::toggle_line_comment("example", 0..0, kind.label())
                    .text
                    .starts_with("# "),
                "{name}"
            );
        }
        assert_eq!(from_path(None), FileKind::PlainText);
        assert_eq!(
            crate::editor::toggle_line_comment(
                "value = 1",
                0..0,
                comment_language(Some(Path::new("source.hs")))
            )
            .text,
            "-- value = 1"
        );
    }

    #[test]
    fn ini_like_files_use_their_own_comment_marker() {
        for (name, expected) in [
            ("server.conf", "# key = 1"),
            ("setup.cfg", "# key = 1"),
            ("Setup.CFG", "# key = 1"),
            ("settings.ini", "; key = 1"),
        ] {
            assert_eq!(from_path(Some(Path::new(name))), FileKind::Ini, "{name}");
            assert_eq!(
                crate::editor::toggle_line_comment(
                    "key = 1",
                    0..0,
                    comment_language(Some(Path::new(name)))
                )
                .text,
                expected,
                "{name}"
            );
        }
    }
}
