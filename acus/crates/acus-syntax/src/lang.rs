// SPDX-License-Identifier: MPL-2.0
//! Owns the languages acus outlines: their names, file extensions and grammars.
use serde::Serialize;
use std::path::Path;

/// Names a language acus can outline. Variants that share a grammar crate share one Cargo feature
/// (TypeScript, Tsx and JavaScript all use `lang-ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Rust,
    Swift,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Markdown,
    Svelte,
    Toml,
    Json,
    Yaml,
    Shell,
    C,
    Css,
    Scss,
    Html,
    Xml,
}

impl Lang {
    /// Lists every language with its name and lowercase extensions. `from_path` and `name` read
    /// only this table, so a language needs exactly one row here.
    pub(crate) const ALL: &[(Lang, &str, &[&str])] = &[
        (Lang::Rust, "rust", &["rs"]),
        (Lang::Swift, "swift", &["swift"]),
        (Lang::TypeScript, "typescript", &["ts", "mts", "cts"]),
        (Lang::Tsx, "tsx", &["tsx"]),
        (Lang::JavaScript, "javascript", &["js", "mjs", "cjs", "jsx"]),
        (Lang::Python, "python", &["py", "pyi"]),
        (Lang::Markdown, "markdown", &["md", "markdown"]),
        (Lang::Svelte, "svelte", &["svelte"]),
        (Lang::Toml, "toml", &["toml"]),
        (Lang::Json, "json", &["json"]),
        (Lang::Yaml, "yaml", &["yaml", "yml"]),
        (Lang::Shell, "shell", &["sh", "bash", "zsh"]),
        (Lang::C, "c", &["c", "h"]),
        (Lang::Css, "css", &["css"]),
        (Lang::Scss, "scss", &["scss"]),
        (Lang::Html, "html", &["html", "htm"]),
        (Lang::Xml, "xml", &["xml", "plist", "xib", "storyboard"]),
    ];

    /// Lists every file extension (lowercase, no dot) of every language.
    pub fn extensions() -> impl Iterator<Item = &'static str> {
        Lang::ALL
            .iter()
            .flat_map(|&(_, _, exts)| exts.iter().copied())
    }

    pub fn from_path(path: &Path) -> Option<Lang> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Lang::ALL
            .iter()
            .find(|(_, _, exts)| exts.contains(&ext.as_str()))
            .map(|&(lang, _, _)| lang)
    }

    #[expect(
        clippy::expect_used,
        reason = "every Lang has a row in Lang::ALL; a test checks it"
    )]
    pub fn name(self) -> &'static str {
        // A missing row would panic; `rows::every_variant_has_a_row` catches it at test time.
        Lang::ALL
            .iter()
            .find(|&&(lang, _, _)| lang == self)
            .map(|&(_, name, _)| name)
            .expect("every Lang has a row in Lang::ALL")
    }

    /// Returns the grammar, or `None` when its Cargo feature is disabled.
    #[cfg_attr(
        all(
            feature = "lang-rust",
            feature = "lang-swift",
            feature = "lang-ts",
            feature = "lang-python",
            feature = "lang-markdown",
            feature = "lang-svelte",
            feature = "lang-data",
            feature = "lang-shell",
            feature = "lang-c",
            feature = "lang-css",
            feature = "lang-scss",
            feature = "lang-html",
            feature = "lang-xml",
            not(windows)
        ),
        expect(
            unreachable_patterns,
            reason = "the _ arm is live only when a grammar is compiled out"
        )
    )]
    pub(crate) fn grammar(self) -> Option<tree_sitter::Language> {
        match self {
            #[cfg(feature = "lang-rust")]
            Lang::Rust => Some(tree_sitter_rust::LANGUAGE.into()),
            #[cfg(feature = "lang-swift")]
            Lang::Swift => Some(tree_sitter_swift::LANGUAGE.into()),
            #[cfg(feature = "lang-ts")]
            Lang::TypeScript => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
            #[cfg(feature = "lang-ts")]
            Lang::Tsx => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
            #[cfg(feature = "lang-ts")]
            Lang::JavaScript => Some(tree_sitter_javascript::LANGUAGE.into()),
            #[cfg(feature = "lang-python")]
            Lang::Python => Some(tree_sitter_python::LANGUAGE.into()),
            #[cfg(feature = "lang-markdown")]
            Lang::Markdown => Some(tree_sitter_md::LANGUAGE.into()),
            #[cfg(feature = "lang-svelte")]
            Lang::Svelte => Some(tree_sitter_svelte_ng::LANGUAGE.into()),
            #[cfg(feature = "lang-data")]
            Lang::Toml => Some(tree_sitter_toml_ng::LANGUAGE.into()),
            #[cfg(feature = "lang-data")]
            Lang::Json => Some(tree_sitter_json::LANGUAGE.into()),
            #[cfg(feature = "lang-data")]
            Lang::Yaml => Some(tree_sitter_yaml::LANGUAGE.into()),
            #[cfg(feature = "lang-shell")]
            Lang::Shell => Some(tree_sitter_bash::LANGUAGE.into()),
            #[cfg(feature = "lang-c")]
            Lang::C => Some(tree_sitter_c::LANGUAGE.into()),
            #[cfg(feature = "lang-css")]
            Lang::Css => Some(tree_sitter_css::LANGUAGE.into()),
            #[cfg(all(feature = "lang-scss", not(windows)))]
            Lang::Scss => Some(tree_sitter_scss::language()),
            #[cfg(feature = "lang-html")]
            Lang::Html => Some(tree_sitter_html::LANGUAGE.into()),
            #[cfg(feature = "lang-xml")]
            Lang::Xml => Some(tree_sitter_xml::LANGUAGE_XML.into()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod rows {
    use super::*;

    /// Panics in `name` on a variant without a row. The exhaustive match makes a new variant fail
    /// to compile here until it is added to the list below.
    #[test]
    fn every_variant_has_a_row() {
        use Lang::*;
        let all = [
            Rust, Swift, TypeScript, Tsx, JavaScript, Python, Markdown, Svelte, Toml, Json, Yaml,
            Shell, C, Css, Scss, Html, Xml,
        ];
        for lang in all {
            match lang {
                Rust | Swift | TypeScript | Tsx | JavaScript | Python | Markdown | Svelte
                | Toml | Json | Yaml | Shell | C | Css | Scss | Html | Xml => {}
            }
            assert!(Lang::ALL.iter().any(|r| r.0 == lang), "{lang:?} has no row");
        }
        assert_eq!(all.len(), Lang::ALL.len());
    }
}

#[cfg(all(
    test,
    feature = "lang-rust",
    feature = "lang-swift",
    feature = "lang-ts",
    feature = "lang-python",
    feature = "lang-markdown",
    feature = "lang-svelte",
    feature = "lang-data",
    feature = "lang-shell",
    feature = "lang-c",
    feature = "lang-css",
    feature = "lang-scss",
    feature = "lang-html",
    feature = "lang-xml",
))]
mod tests {
    use super::*;

    /// Checks each variant, since the `_ => return None` fallback would otherwise hide one with no
    /// grammar arm.
    #[test]
    fn every_listed_language_has_a_grammar() {
        for &(lang, name, _) in Lang::ALL {
            // No SCSS grammar on Windows; see Cargo.toml.
            if cfg!(windows) && lang == Lang::Scss {
                continue;
            }
            assert!(lang.grammar().is_some(), "no grammar arm for {name}");
        }
    }
}
