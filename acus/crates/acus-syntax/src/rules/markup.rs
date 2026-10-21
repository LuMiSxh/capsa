// SPDX-License-Identifier: MPL-2.0
//! Owns the outline rules for the style and markup languages: CSS, HTML and XML.

use super::{Class, follows_key, text};
use tree_sitter::Node;

/// Lists the CSS and SCSS symbols: rule sets, `@media`/`@supports` blocks with their rules nested
/// inside, `@keyframes`, and `--x` custom properties, but only when `:root` declares them.
pub(super) fn css(n: Node, src: &[u8]) -> Option<Class> {
    match n.kind() {
        "rule_set" => Some(Class {
            kind: "rule",
            name: css_name(css_selectors(n, src)?),
            detail: None,
            leaf: false,
        }),
        "media_statement" => css_block("media", n, src),
        "supports_statement" => css_block("supports", n, src),
        "keyframes_statement" => {
            let mut cur = n.walk();
            let name = n
                .named_children(&mut cur)
                .find(|k| k.kind() == "keyframes_name")?;
            Some(Class {
                kind: "keyframes",
                name: text(name, src).to_owned(),
                detail: None,
                leaf: true,
            })
        }
        "declaration" => {
            let mut cur = n.walk();
            let prop = n
                .named_children(&mut cur)
                .find(|k| k.kind() == "property_name")?;
            let name = text(prop, src);
            // Declaration in a block of a rule set; a top-level one has no such parent.
            let rule = n.parent()?.parent()?;
            if !name.starts_with("--") || rule.kind() != "rule_set" {
                return None;
            }
            if css_name(css_selectors(rule, src)?) != ":root" {
                return None;
            }
            Some(Class {
                kind: "var",
                name: name.to_owned(),
                detail: None,
                leaf: true,
            })
        }
        _ => None,
    }
}

/// Returns the selector text of a rule set, as written.
fn css_selectors<'a>(rule: Node, src: &'a [u8]) -> Option<&'a str> {
    let mut cur = rule.walk();
    let sel = rule
        .named_children(&mut cur)
        .find(|k| k.kind() == "selectors")?;
    Some(text(sel, src))
}

/// Returns a selector or query on one line, cut to 60 characters.
fn css_name(s: &str) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= 60 {
        return flat;
    }
    let head: String = flat.chars().take(57).collect();
    format!("{head}...")
}

/// Names `@media QUERY { ... }` and `@supports QUERY { ... }` by their query.
fn css_block(kind: &'static str, n: Node, src: &[u8]) -> Option<Class> {
    let mut cur = n.walk();
    let block = n.named_children(&mut cur).find(|k| k.kind() == "block")?;
    let head = std::str::from_utf8(src.get(n.start_byte()..block.start_byte())?).ok()?;
    let query = head.trim().trim_start_matches('@').trim_start_matches(kind);
    Some(Class {
        kind,
        name: css_name(query),
        detail: None,
        leaf: false,
    })
}

/// Lists the HTML symbols: elements with an `id` (`div#main`), and `script`, `style`, `template`
/// and the `section`, `main`, `nav`, `header`, `footer` landmarks. Other elements only nest.
pub(super) fn html(n: Node, src: &[u8]) -> Option<Class> {
    if !matches!(n.kind(), "element" | "script_element" | "style_element") {
        return None;
    }
    let mut cur = n.walk();
    let start = n
        .named_children(&mut cur)
        .find(|k| matches!(k.kind(), "start_tag" | "self_closing_tag"))?;
    let mut cur = start.walk();
    let tag = start
        .named_children(&mut cur)
        .find(|k| k.kind() == "tag_name")
        .map(|t| text(t, src).to_ascii_lowercase())?;
    let mut cur = start.walk();
    let id = start
        .named_children(&mut cur)
        .filter(|a| a.kind() == "attribute")
        .find_map(|a| {
            let (key, value) = text(a, src).split_once('=')?;
            let value = value.trim().trim_matches(['"', '\'']);
            (key.trim().eq_ignore_ascii_case("id") && !value.is_empty()).then(|| value.to_owned())
        });
    let kind = match tag.as_str() {
        "section" => "section",
        "main" => "main",
        "nav" => "nav",
        "header" => "header",
        "footer" => "footer",
        "script" => "script",
        "style" => "style",
        "template" => "template",
        _ => "element",
    };
    let name = match id {
        Some(id) => format!("{tag}#{id}"),
        None if kind != "element" => tag,
        None => return None,
    };
    Some(Class {
        kind,
        name,
        detail: None,
        leaf: false,
    })
}

/// Lists the XML and plist symbols: elements down to depth 2, named by tag plus `id`, `name`
/// or `key`. A plist `<key>` is named by its text, and the value element right after it is not
/// listed.
pub(super) fn xml(n: Node, src: &[u8]) -> Option<Class> {
    if n.kind() != "element" {
        return None;
    }
    let mut depth = 0;
    let mut up = n.parent();
    while let Some(p) = up {
        if p.kind() == "element" {
            depth += 1;
        }
        up = p.parent();
    }
    if depth > 2 {
        return None;
    }
    let tag = xml_name_of(n, src)?;
    if tag == "key" {
        let t = text(n, src);
        let value = t.get(t.find('>')? + 1..t.rfind("</")?)?.trim();
        return (!value.is_empty()).then(|| Class {
            kind: "key",
            name: value.to_owned(),
            detail: None,
            leaf: true,
        });
    }
    if follows_key(n, src) {
        return None;
    }
    let mut cur = n.walk();
    let start = n
        .named_children(&mut cur)
        .find(|k| matches!(k.kind(), "STag" | "EmptyElemTag"))?;
    let mut cur = start.walk();
    let attrs: Vec<(&str, &str)> = start
        .named_children(&mut cur)
        .filter(|a| a.kind() == "Attribute")
        .filter_map(|a| {
            let mut c = a.walk();
            let key = a.named_children(&mut c).find(|k| k.kind() == "Name")?;
            let value = a.named_children(&mut c).find(|k| k.kind() == "AttValue")?;
            Some((text(key, src), text(value, src).trim_matches(['"', '\''])))
        })
        .collect();
    let id = ["id", "name", "key"]
        .iter()
        .find_map(|want| attrs.iter().find(|(k, _)| k == want).map(|(_, v)| *v));
    let name = match id {
        Some(v) if !v.is_empty() => format!("{tag}#{v}"),
        _ => tag.to_owned(),
    };
    Some(Class {
        kind: "element",
        name,
        detail: None,
        leaf: false,
    })
}

/// Returns the `Name` of an XML element's start tag.
pub(super) fn xml_name_of<'a>(elem: Node, src: &'a [u8]) -> Option<&'a str> {
    let mut cur = elem.walk();
    let start = elem
        .named_children(&mut cur)
        .find(|k| matches!(k.kind(), "STag" | "EmptyElemTag"))?;
    let mut cur = start.walk();
    let name = start
        .named_children(&mut cur)
        .find(|k| k.kind() == "Name")?;
    Some(text(name, src))
}
