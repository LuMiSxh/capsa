// SPDX-License-Identifier: MPL-2.0
//! Owns which tree-sitter nodes become outline symbols, per language.

mod markup;

use crate::Lang;
use markup::{css, html, xml, xml_name_of};
use tree_sitter::Node;

/// Lists every kind a rule below emits, sorted. `outline --kind` accepts exactly these; the
/// `every_emitted_kind_is_listed` test in tests/outline.rs fails when a rule emits one that is
/// missing here.
pub const KINDS: &[&str] = &[
    "actor",
    "array",
    "class",
    "const",
    "def",
    "element",
    "enum",
    "extension",
    "fn",
    "footer",
    "func",
    "function",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "impl",
    "interface",
    "key",
    "keyframes",
    "macro",
    "main",
    "media",
    "method",
    "mod",
    "namespace",
    "nav",
    "protocol",
    "rule",
    "script",
    "section",
    "static",
    "struct",
    "style",
    "supports",
    "table",
    "template",
    "trait",
    "type",
    "typealias",
    "typedef",
    "union",
    "var",
];

pub(crate) struct Class {
    pub kind: &'static str,
    pub name: String,
    pub detail: Option<String>,
    /// Leaves are not searched for nested symbols (function bodies, structs, keys).
    pub leaf: bool,
}

pub(crate) fn text<'a>(n: Node, src: &'a [u8]) -> &'a str {
    n.utf8_text(src).unwrap_or("")
}

fn field<'a>(n: Node, f: &str, src: &'a [u8]) -> Option<&'a str> {
    n.child_by_field_name(f).map(|c| text(c, src))
}

fn named(kind: &'static str, n: Node, src: &[u8], leaf: bool) -> Option<Class> {
    Some(Class {
        kind,
        name: field(n, "name", src)?.trim().to_owned(),
        detail: None,
        leaf,
    })
}

pub(crate) fn classify(lang: Lang, n: Node, src: &[u8]) -> Option<Class> {
    match lang {
        Lang::Rust => rust(n, src),
        Lang::Swift => swift(n, src),
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript | Lang::Svelte => ts(n, src),
        Lang::Python => python(n, src),
        Lang::Markdown => markdown(n, src),
        Lang::Toml | Lang::Json | Lang::Yaml => data(n, src),
        Lang::Shell => shell(n, src),
        Lang::C => c(n, src),
        Lang::Css | Lang::Scss => css(n, src),
        Lang::Html => html(n, src),
        Lang::Xml => xml(n, src),
    }
}

fn rust(n: Node, src: &[u8]) -> Option<Class> {
    let leaf = |k: &'static str| named(k, n, src, true);
    match n.kind() {
        "function_item" | "function_signature_item" => leaf("fn"),
        "struct_item" => leaf("struct"),
        "union_item" => leaf("union"),
        "enum_item" => leaf("enum"),
        "const_item" => leaf("const"),
        "static_item" => leaf("static"),
        "type_item" => leaf("type"),
        "macro_definition" => leaf("macro"),
        "mod_item" => named("mod", n, src, false),
        "trait_item" => named("trait", n, src, false),
        "impl_item" => {
            let ty = field(n, "type", src)?;
            let name = ty.split('<').next().unwrap_or(ty).trim().to_owned();
            let detail = field(n, "trait", src).map(|t| format!("{t} for {ty}"));
            Some(Class {
                kind: "impl",
                name,
                detail,
                leaf: false,
            })
        }
        _ => None,
    }
}

fn swift(n: Node, src: &[u8]) -> Option<Class> {
    let leaf = |k: &'static str| named(k, n, src, true);
    match n.kind() {
        "class_declaration" => {
            let kind = match field(n, "declaration_kind", src)? {
                "struct" => "struct",
                "enum" => "enum",
                "extension" => "extension",
                "actor" => "actor",
                _ => "class",
            };
            named(kind, n, src, false)
        }
        "protocol_declaration" => named("protocol", n, src, false),
        "function_declaration" | "protocol_function_declaration" => leaf("func"),
        "init_declaration" => Some(Class {
            kind: "func",
            name: "init".into(),
            detail: None,
            leaf: true,
        }),
        "property_declaration" => leaf("var"),
        "typealias_declaration" => leaf("typealias"),
        _ => None,
    }
}

fn ts(n: Node, src: &[u8]) -> Option<Class> {
    let leaf = |k: &'static str| named(k, n, src, true);
    match n.kind() {
        "class_declaration" | "abstract_class_declaration" | "class" => {
            named("class", n, src, false)
        }
        "internal_module" => named("namespace", n, src, false),
        "method_definition" => leaf("method"),
        "function_declaration" | "generator_function_declaration" => leaf("function"),
        "interface_declaration" => leaf("interface"),
        "type_alias_declaration" => leaf("type"),
        "enum_declaration" => leaf("enum"),
        "variable_declarator" => {
            let v = n.child_by_field_name("value")?.kind();
            if !matches!(v, "arrow_function" | "function_expression" | "function") {
                return None;
            }
            leaf("function")
        }
        _ => None,
    }
}

fn python(n: Node, src: &[u8]) -> Option<Class> {
    match n.kind() {
        "class_definition" => named("class", n, src, false),
        "function_definition" => named("def", n, src, true),
        _ => None,
    }
}

fn shell(n: Node, src: &[u8]) -> Option<Class> {
    match n.kind() {
        "function_definition" => named("fn", n, src, true),
        _ => None,
    }
}

fn c(n: Node, src: &[u8]) -> Option<Class> {
    let leaf = |kind: &'static str, name: &str| Class {
        kind,
        name: name.to_owned(),
        detail: None,
        leaf: true,
    };
    match n.kind() {
        // Prototypes are `declaration`s too, so a header lists its functions.
        "function_definition" | "declaration" => Some(leaf("fn", c_function_name(n, src)?)),
        "struct_specifier" => c_tag("struct", n, src),
        "union_specifier" => c_tag("union", n, src),
        "enum_specifier" => c_tag("enum", n, src),
        "type_definition" => Some(leaf("typedef", c_typedef_name(n, src)?)),
        "preproc_def" | "preproc_function_def" => named("macro", n, src, true),
        _ => None,
    }
}

/// Lists a specifier only with a body, so `struct S;` forward declarations are skipped.
fn c_tag(kind: &'static str, n: Node, src: &[u8]) -> Option<Class> {
    n.child_by_field_name("body")?;
    named(kind, n, src, true)
}

/// Returns the name of a function, through pointer declarators (`char *f(void)`).
fn c_function_name<'a>(n: Node, src: &'a [u8]) -> Option<&'a str> {
    let mut d = n.child_by_field_name("declarator")?;
    while d.kind() == "pointer_declarator" {
        d = d.child_by_field_name("declarator")?;
    }
    if d.kind() != "function_declarator" {
        return None;
    }
    let id = d.child_by_field_name("declarator")?;
    (id.kind() == "identifier").then(|| text(id, src))
}

fn c_typedef_name<'a>(n: Node, src: &'a [u8]) -> Option<&'a str> {
    let mut d = n.child_by_field_name("declarator")?;
    while matches!(d.kind(), "pointer_declarator" | "array_declarator") {
        d = d.child_by_field_name("declarator")?;
    }
    (d.kind() == "type_identifier").then(|| text(d, src))
}

fn markdown(n: Node, src: &[u8]) -> Option<Class> {
    // ATX headings open a section; setext headings (`Title\n---`) stand alone in one.
    let heading = match n.kind() {
        "section" => {
            let mut cur = n.walk();
            n.named_children(&mut cur)
                .find(|c| c.kind() == "atx_heading")?
        }
        "setext_heading" => n,
        _ => return None,
    };
    let mut c2 = heading.walk();
    let level = heading.named_children(&mut c2).find_map(|c| {
        let k = c.kind();
        k.strip_prefix("atx_h")
            .and_then(|k| k.strip_suffix("_marker"))
            .or_else(|| k.strip_prefix("setext_h")?.strip_suffix("_underline"))
    });
    let kind = match level? {
        "1" => "h1",
        "2" => "h2",
        "3" => "h3",
        "4" => "h4",
        "5" => "h5",
        _ => "h6",
    };
    let name = field(heading, "heading_content", src)?.trim().to_owned();
    Some(Class {
        kind,
        name,
        detail: None,
        leaf: n.kind() == "setext_heading",
    })
}

/// Lists tables and keys; keys holding tables or mappings are searched for nested keys.
fn data(n: Node, src: &[u8]) -> Option<Class> {
    let (kind, key) = match n.kind() {
        "table" => ("table", n.named_child(0)?),
        "table_array_element" => ("array", n.named_child(0)?),
        // TOML pairs have no `key` field; the key is their first child.
        "pair" | "block_mapping_pair" => (
            "key",
            n.child_by_field_name("key").or_else(|| n.named_child(0))?,
        ),
        _ => return None,
    };
    let name = text(key, src)
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_owned();
    Some(Class {
        kind,
        name,
        detail: None,
        leaf: false,
    })
}

/// Reports whether the closest element before `n` among its siblings is a plist `<key>`.
fn follows_key(n: Node, src: &[u8]) -> bool {
    let mut prev = n.prev_sibling();
    while let Some(p) = prev {
        if p.kind() == "element" {
            return xml_name_of(p, src) == Some("key");
        }
        prev = p.prev_sibling();
    }
    false
}
