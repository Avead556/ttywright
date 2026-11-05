macro_rules! grammar {
    ($feature:literal, $language:expr) => {
        #[cfg(feature = $feature)]
        #[unsafe(no_mangle)]
        pub extern "C" fn ttywright_grammar() -> *const () {
            unsafe { $language.into_raw()() }
        }
    };
}

grammar!("rust", tree_sitter_rust::LANGUAGE);
grammar!("toml", tree_sitter_toml_ng::LANGUAGE);
grammar!("markdown", tree_sitter_md::LANGUAGE);
grammar!("json", tree_sitter_json::LANGUAGE);
grammar!("php", tree_sitter_php::LANGUAGE_PHP);
grammar!("javascript", tree_sitter_javascript::LANGUAGE);
grammar!("typescript", tree_sitter_typescript::LANGUAGE_TYPESCRIPT);
grammar!("tsx", tree_sitter_typescript::LANGUAGE_TSX);
grammar!("java", tree_sitter_java::LANGUAGE);
grammar!("go", tree_sitter_go::LANGUAGE);
grammar!("python", tree_sitter_python::LANGUAGE);
grammar!("bash", tree_sitter_bash::LANGUAGE);
