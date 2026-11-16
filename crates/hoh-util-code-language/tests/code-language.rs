use hoh_util_code_language::{
    CODE_HIGHLIGHT_EXTENSIONS, language_for_path, read_lang_hint_for_path,
};
use std::collections::HashSet;

#[test]
fn language_for_path_resolves_filename_to_language() {
    for (path, language) in [
        ("build.bat", "bat"),
        ("build.cmd", "bat"),
        ("deploy.ps1", "powershell"),
        ("module.psm1", "powershell"),
        ("data.psd1", "powershell"),
        ("config.fish", "fish"),
        ("app.properties", "ini"),
        ("app.conf", "ini"),
        ("app.cfg", "ini"),
        (".env", "dotenv"),
        ("server.log", "log"),
        ("change.diff", "diff"),
        ("fix.patch", "diff"),
        ("api.http", "http"),
        ("notebook.ipynb", "json"),
        ("table.csv", "csv"),
        ("guide.rst", "rst"),
        ("paper.tex", "latex"),
        ("style.sty", "latex"),
        ("doc.cls", "latex"),
        ("refs.bib", "bibtex"),
        ("Info.plist", "xml"),
        ("logo.svg", "xml"),
        ("manual.adoc", "asciidoc"),
        ("analysis.r", "r"),
        ("model.jl", "julia"),
        ("main.dart", "dart"),
        ("Main.scala", "scala"),
        ("core.clj", "clojure"),
        ("ui.cljs", "clojure"),
        ("data.edn", "clojure"),
        ("app.erl", "erlang"),
        ("app.hrl", "erlang"),
        ("app.ex", "elixir"),
        ("app.exs", "elixir"),
        ("Main.hs", "haskell"),
        ("Types.fs", "fsharp"),
        ("Types.fsi", "fsharp"),
        ("script.fsx", "fsharp"),
        ("Form.vb", "vb"),
        ("script.pl", "perl"),
        ("Module.pm", "perl"),
        ("top.v", "verilog"),
        ("top.sv", "system-verilog"),
        ("defs.svh", "system-verilog"),
        ("schema.graphql", "graphql"),
        ("query.gql", "graphql"),
        ("message.proto", "proto"),
        ("main.tf", "hcl"),
        ("terraform.tfvars", "hcl"),
        ("stack.hcl", "hcl"),
        ("build.groovy", "groovy"),
        ("flake.nix", "nix"),
        ("App.vue", "vue"),
        ("App.svelte", "svelte"),
        ("build.mk", "make"),
        ("CMakeLists.cmake", "cmake"),
        ("build.gradle", "groovy"),
    ] {
        assert_eq!(language_for_path(path), Some(language), "{path}");
    }
    assert_eq!(language_for_path("Makefile"), None);
}

#[test]
fn resolves_extensions_preview_and_read_already_agreed_on() {
    for (path, language) in [
        ("src/a.ts", "typescript"),
        ("src/a.TSX", "typescript"),
        ("/abs/module.mjs", "javascript"),
        ("conf.yml", "yaml"),
        ("README.md", "markdown"),
        (r"C:\src\main.rs", "rust"),
    ] {
        assert_eq!(language_for_path(path), Some(language));
    }
}

#[test]
fn resolves_every_preview_suffix_beyond_persisted_read_hints() {
    for extension in [
        "jsonl", "ndjson", "pyw", "pyi", "rake", "gemspec", "hh", "hxx", "kts", "xhtml", "xsd",
        "xsl", "xslt",
    ] {
        assert!(
            language_for_path(&format!("file.{extension}")).is_some(),
            "{extension}"
        );
    }
}

#[test]
fn is_case_insensitive_and_accepts_windows_separators() {
    for (path, language) in [
        (r"C:\path\X.PS1", "powershell"),
        (r"C:\Proj\Build.CMD", "bat"),
        (r"dir\sub\Main.SCALA", "scala"),
    ] {
        assert_eq!(language_for_path(path), Some(language));
    }
}

#[test]
fn reads_suffix_after_last_path_segment_and_last_dot() {
    for path in [
        "a.py.bak",
        "archive.tar.gz",
        "/dir.py/plain",
        "dir.ts/README",
    ] {
        assert_eq!(language_for_path(path), None);
    }
}

#[test]
fn returns_none_for_dotfile_extensionless_trailing_dot_and_unknown_suffix() {
    for path in [
        ".gitignore",
        "/etc/hosts",
        "trailingdot.",
        "data.unknownext",
    ] {
        assert_eq!(language_for_path(path), None);
    }
}

#[test]
fn returns_none_for_object_prototype_keys() {
    for path in [
        "foo.constructor",
        "foo.__proto__",
        "foo.toString",
        "foo.hasOwnProperty",
    ] {
        assert_eq!(language_for_path(path), None);
    }
}

#[test]
fn leaves_certificate_and_lock_extensions_unlisted() {
    for extension in ["pem", "crt", "key", "cer", "lock"] {
        assert_eq!(
            language_for_path(&format!("secret.{extension}")),
            None,
            "{extension}"
        );
    }
}

#[test]
fn leaves_tsv_unlisted_without_highlighter_grammar() {
    assert_eq!(language_for_path("table.tsv"), None);
}

#[test]
fn keeps_read_card_short_hints_recorded_sessions_already_hold() {
    for (path, hint) in [
        ("src/a.ts", "ts"),
        ("src/a.tsx", "tsx"),
        ("src/a.mts", "ts"),
        ("src/a.cts", "ts"),
        ("src/a.js", "js"),
        ("src/a.jsx", "jsx"),
        ("src/a.mjs", "js"),
        ("src/a.cjs", "js"),
        ("src/a.jsonc", "json"),
        ("src/a.cc", "cpp"),
        ("src/a.hpp", "cpp"),
        ("src/a.sh", "sh"),
        ("src/a.kt", "kotlin"),
        ("README.md", "md"),
        ("README.markdown", "md"),
        ("page.htm", "html"),
    ] {
        assert_eq!(read_lang_hint_for_path(path), Some(hint));
    }
}

#[test]
fn uses_language_short_id_for_suffix_without_persisted_value() {
    for (path, hint) in [
        ("build.ps1", "ps1"),
        ("table.csv", "csv"),
        (".env", "env"),
        ("infra.tf", "tf"),
        ("paper.tex", "tex"),
        ("server.log", "log"),
        ("message.proto", "proto"),
        ("nomad.hcl", "hcl"),
        ("main.tf", "tf"),
        ("terraform.tfvars", "tfvars"),
        ("build.gradle", "gradle"),
        ("build.groovy", "groovy"),
    ] {
        assert_eq!(read_lang_hint_for_path(path), Some(hint));
    }
}

#[test]
fn persists_short_id_for_every_suffix() {
    // Independently transcribed values from the source test's SHORT_BY_LANGUAGE.
    let allowed: HashSet<_> = [
        "ts", "js", "sh", "fish", "json", "csv", "py", "rb", "go", "rs", "java", "c", "cpp", "cs",
        "kotlin", "swift", "php", "yaml", "toml", "ini", "env", "log", "diff", "http", "md", "mdx",
        "rst", "tex", "bib", "adoc", "html", "css", "scss", "less", "sql", "xml", "lua", "bat",
        "ps1", "r", "jl", "dart", "scala", "clj", "erl", "ex", "hs", "fs", "vb", "pl", "v", "sv",
        "graphql", "proto", "hcl", "nix", "vue", "svelte", "make", "cmake", "groovy", "tsx", "jsx",
        "tf", "tfvars", "gradle",
    ]
    .into_iter()
    .collect();
    for extension in CODE_HIGHLIGHT_EXTENSIONS {
        let hint = read_lang_hint_for_path(&format!("file.{extension}"));
        assert!(hint.is_some(), "{extension}");
        assert!(allowed.contains(hint.unwrap_or("")), "{extension}");
    }
}

#[test]
fn read_hint_is_none_when_shared_table_does_not_recognize_suffix() {
    for path in [
        ".gitignore",
        "/etc/hosts",
        "trailingdot.",
        "data.unknownext",
        "foo.constructor",
    ] {
        assert_eq!(read_lang_hint_for_path(path), None, "{path}");
    }
}

#[test]
fn lists_every_recognized_suffix_exactly_once() {
    let unique: HashSet<_> = CODE_HIGHLIGHT_EXTENSIONS.iter().collect();
    assert_eq!(unique.len(), CODE_HIGHLIGHT_EXTENSIONS.len());
    assert!(CODE_HIGHLIGHT_EXTENSIONS.len() > 100);
}

#[test]
fn resolves_each_listed_suffix_back_to_language() {
    for extension in CODE_HIGHLIGHT_EXTENSIONS {
        assert!(
            language_for_path(&format!("file.{extension}")).is_some(),
            "{extension}"
        );
    }
}
