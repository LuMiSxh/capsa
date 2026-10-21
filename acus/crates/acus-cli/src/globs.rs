// SPDX-License-Identifier: MPL-2.0
//! Glob helpers shared by `find` and `files`.

/// Adds the include globs for `-t` types to `include`.
pub(crate) fn add_type_globs(types: &[String], include: &mut Vec<String>) {
    for t in types {
        // Not acus_syntax::Lang: -t ts also covers .tsx, which Lang gives its own Tsx variant.
        let exts: &[&str] = match t.as_str() {
            "rust" => &["rs"],
            "py" | "python" => &["py", "pyi"],
            "ts" | "typescript" => &["ts", "tsx", "mts", "cts"],
            "js" | "javascript" => &["js", "jsx", "mjs", "cjs"],
            "md" | "markdown" => &["md", "markdown"],
            "yaml" | "yml" => &["yaml", "yml"],
            "cpp" | "c++" => &["cpp", "cc", "cxx", "hpp", "hh", "h"],
            other => {
                include.push(format!("*.{other}"));
                continue;
            }
        };
        include.extend(exts.iter().map(|e| format!("*.{e}")));
    }
}

/// The globs and search roots named in the hint for a search or listing with no hits, with the
/// rule that decides what a glob matches. None without a glob.
pub(crate) fn searched_globs(globs: &[String], paths: &[String]) -> Option<String> {
    (!globs.is_empty()).then(|| {
        let shown: Vec<String> = globs.iter().map(|g| format!("-g '{g}'")).collect();
        let paths = if paths.is_empty() {
            ".".to_owned()
        } else {
            paths.join(" ")
        };
        let rule = if globs.iter().any(|g| !g.contains('/')) {
            "a glob without / matches a file name at any depth"
        } else {
            "a glob with / is relative to the search root"
        };
        format!("{} in {paths}; {rule}", shown.join(" "))
    })
}
