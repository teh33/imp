pub(super) fn language_for_extension(ext: &str) -> Option<tree_sitter::Language> {
    let language = match ext {
        "sh" | "bash" | "zsh" | "fish" => tree_sitter_bash::LANGUAGE.into(),
        "ex" | "exs" => tree_sitter_elixir::LANGUAGE.into(),
        "rb" => tree_sitter_ruby::LANGUAGE.into(),
        "ml" | "mli" => tree_sitter_ocaml::LANGUAGE_OCAML.into(),
        "pl" | "pm" | "t" => tree_sitter_perl::LANGUAGE.into(),
        "lua" | "luau" => tree_sitter_lua::LANGUAGE.into(),
        "zig" | "zon" => tree_sitter_zig::LANGUAGE.into(),
        "odin" => tree_sitter_odin::LANGUAGE.into(),
        "swift" => tree_sitter_swift::LANGUAGE.into(),
        "java" => tree_sitter_java::LANGUAGE.into(),
        "c" | "h" => tree_sitter_c::LANGUAGE.into(),
        "cs" => tree_sitter_c_sharp::LANGUAGE.into(),
        "cc" | "cpp" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" => {
            tree_sitter_cpp::LANGUAGE.into()
        }
        "php" => tree_sitter_php::LANGUAGE_PHP.into(),
        "scala" | "sc" => tree_sitter_scala::LANGUAGE.into(),
        "dart" => tree_sitter_dart::LANGUAGE.into(),
        _ => return None,
    };
    Some(language)
}
