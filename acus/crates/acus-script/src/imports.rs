// SPDX-License-Identifier: MPL-2.0
//! Lets a program start with static `import` lines: the body of an async function has none, so
//! each leading import becomes a dynamic `import()` on the same line, which keeps line numbers.

use regex::Regex;
use std::borrow::Cow;
use std::sync::LazyLock;

#[expect(clippy::unwrap_used, reason = "the pattern is a literal that compiles")]
static IMPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^(\s*)import\s+(?:\*\s*as\s+(\w+)|(\w+)(?:\s*,\s*\{([^}]*)\})?|\{([^}]*)\})\s*from\s*("[^"\n]+"|'[^'\n]+')\s*(?:;(.*))?$"#,
    )
    .unwrap()
});

/// Returns `code` with its leading import lines turned into `await import()` declarations. A
/// program without leading imports is returned as it is.
pub(crate) fn dynamic(code: &str) -> Cow<'_, str> {
    let mut changed = false;
    let mut out = String::with_capacity(code.len());
    let mut leading = true;
    for line in code.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        let end = &line[text.len()..];
        let trimmed = text.trim_start();
        if leading && trimmed.starts_with("import ") {
            if let Some(parts) = IMPORT.captures(text) {
                let indent = &parts[1];
                let spec = &parts[6];
                let named = |names: &str| names.replace(" as ", ": ");
                let declaration = if let Some(ns) = parts.get(2) {
                    format!("const {} = await import({spec});", ns.as_str())
                } else if let Some(name) = parts.get(3) {
                    match parts.get(4) {
                        Some(names) => format!(
                            "const {{default: {}, {}}} = await import({spec});",
                            name.as_str(),
                            named(names.as_str())
                        ),
                        None => {
                            format!("const {} = (await import({spec})).default;", name.as_str())
                        }
                    }
                } else {
                    format!("const {{{}}} = await import({spec});", named(&parts[5]))
                };
                out.push_str(indent);
                out.push_str(&declaration);
                out.push_str(parts.get(7).map_or("", |rest| rest.as_str()));
                out.push_str(end);
                changed = true;
                continue;
            }
        } else if !trimmed.is_empty() && !trimmed.starts_with("//") {
            leading = false;
        }
        out.push_str(line);
    }
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(code)
    }
}

#[cfg(test)]
mod tests {
    use super::dynamic;

    #[test]
    fn leading_imports_become_dynamic_imports_on_the_same_lines() {
        let code = concat!(
            "// tools\nimport gh from \"mcp:github\";\nimport {a, b as c} from 'x:y'\nimport d, {e} from 'x:w'\n",
            "import * as ns from \"x:z\"; print(ns)\nreturn gh;\nimport late from \"x:late\"\n"
        );
        assert_eq!(
            dynamic(code),
            concat!(
                "// tools\nconst gh = (await import(\"mcp:github\")).default;\n",
                "const {a, b: c} = await import('x:y');\nconst {default: d, e} = await import('x:w');\nconst ns = await import(\"x:z\"); print(ns)\n",
                "return gh;\nimport late from \"x:late\"\n"
            )
        );
    }
}
