//! Port of `@deepseek-ai/dsh-util-code-language` (`packages/util/code-language`).

// Keep the public suffix list in the insertion order of the source Map.
macro_rules! languages {
    ($($language:literal => $short:literal: [$($extension:literal),+]),+ $(,)?) => {
        const LANGUAGE_EXTENSIONS: &[(&str, &str, &[&str])] = &[
            $(($language, $short, &[$($extension),+])),+
        ];

        /// Every recognized suffix, once each, in the source table's order.
        pub const CODE_HIGHLIGHT_EXTENSIONS: &[&str] = &[$($($extension),+),+];
    };
}

languages! {
    "typescript" => "ts": ["ts", "tsx", "mts", "cts"],
    "javascript" => "js": ["js", "jsx", "mjs", "cjs"],
    "shellscript" => "sh": ["sh", "bash", "zsh"],
    "fish" => "fish": ["fish"],
    "json" => "json": ["json", "jsonc", "jsonl", "ndjson", "ipynb"],
    "csv" => "csv": ["csv"],
    "python" => "py": ["py", "pyw", "pyi"],
    "ruby" => "rb": ["rb", "rake", "gemspec"],
    "go" => "go": ["go"],
    "rust" => "rs": ["rs"],
    "java" => "java": ["java"],
    "c" => "c": ["c", "h"],
    "cpp" => "cpp": ["cc", "cpp", "cxx", "hh", "hpp", "hxx"],
    "csharp" => "cs": ["cs"],
    "kotlin" => "kotlin": ["kt", "kts"],
    "swift" => "swift": ["swift"],
    "php" => "php": ["php"],
    "yaml" => "yaml": ["yaml", "yml"],
    "toml" => "toml": ["toml"],
    "ini" => "ini": ["ini", "conf", "cfg", "properties"],
    "dotenv" => "env": ["env"],
    "log" => "log": ["log"],
    "diff" => "diff": ["diff", "patch"],
    "http" => "http": ["http"],
    "markdown" => "md": ["md", "markdown"],
    "mdx" => "mdx": ["mdx"],
    "rst" => "rst": ["rst"],
    "latex" => "tex": ["tex", "sty", "cls"],
    "bibtex" => "bib": ["bib"],
    "asciidoc" => "adoc": ["adoc"],
    "html" => "html": ["html", "htm", "xhtml"],
    "css" => "css": ["css"],
    "scss" => "scss": ["scss"],
    "less" => "less": ["less"],
    "sql" => "sql": ["sql"],
    "xml" => "xml": ["xml", "xsd", "xsl", "xslt", "plist", "svg"],
    "lua" => "lua": ["lua"],
    "bat" => "bat": ["bat", "cmd"],
    "powershell" => "ps1": ["ps1", "psm1", "psd1"],
    "r" => "r": ["r"],
    "julia" => "jl": ["jl"],
    "dart" => "dart": ["dart"],
    "scala" => "scala": ["scala"],
    "clojure" => "clj": ["clj", "cljs", "edn"],
    "erlang" => "erl": ["erl", "hrl"],
    "elixir" => "ex": ["ex", "exs"],
    "haskell" => "hs": ["hs"],
    "fsharp" => "fs": ["fs", "fsi", "fsx"],
    "vb" => "vb": ["vb"],
    "perl" => "pl": ["pl", "pm"],
    "verilog" => "v": ["v"],
    "system-verilog" => "sv": ["sv", "svh"],
    "graphql" => "graphql": ["graphql", "gql"],
    "proto" => "proto": ["proto"],
    "hcl" => "hcl": ["tf", "tfvars", "hcl"],
    "nix" => "nix": ["nix"],
    "vue" => "vue": ["vue"],
    "svelte" => "svelte": ["svelte"],
    "make" => "make": ["makefile", "mk"],
    "cmake" => "cmake": ["cmake"],
    "groovy" => "groovy": ["gradle", "groovy"],
}

fn extension_for_path(path: &str) -> Option<String> {
    let base = path.rsplit(['/', '\\']).next()?;
    let (_, extension) = base.rsplit_once('.')?;
    Some(extension.to_lowercase())
}

/// Return the canonical highlighting grammar for the final filename suffix.
pub fn language_for_path(path: &str) -> Option<&'static str> {
    let extension = extension_for_path(path)?;
    LANGUAGE_EXTENSIONS
        .iter()
        .find_map(|(language, _, extensions)| {
            extensions
                .contains(&extension.as_str())
                .then_some(*language)
        })
}

/// Return the read card's persisted short hint, preserving suffix overrides.
pub fn read_lang_hint_for_path(path: &str) -> Option<&'static str> {
    let extension = extension_for_path(path)?;
    let (_, short, _) = LANGUAGE_EXTENSIONS
        .iter()
        .find(|(_, _, extensions)| extensions.contains(&extension.as_str()))?;
    Some(match extension.as_str() {
        "tsx" => "tsx",
        "jsx" => "jsx",
        "tf" => "tf",
        "tfvars" => "tfvars",
        "gradle" => "gradle",
        _ => short,
    })
}
