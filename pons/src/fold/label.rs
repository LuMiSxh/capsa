// SPDX-License-Identifier: MPL-2.0
//! Names a background subagent in the roster from the input of its Agent tool call.

use serde_json::Value;

use super::{clip_head, one_line};

const DEFAULT_TYPE: &str = "general-purpose";
const MAX_PROMPT_CHARS: usize = 60;
const MAX_LABEL_CHARS: usize = 100;
/// ASCII only: pons emits nothing else, see `everything_pons_emits_is_ascii`.
const SEPARATOR: &str = " - ";

/// Returns `<subagent_type> - <description>`, with the `model` input in parentheses after
/// the type. Without a description, the first line of the prompt is used, then `fallback`.
pub(super) fn subagent_label(input: &Value, fallback: &str) -> String {
    let text = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .map(ascii_line)
            .filter(|s| !s.is_empty())
    };
    let kind = text("subagent_type").unwrap_or_else(|| DEFAULT_TYPE.to_owned());
    let kind = match text("model") {
        Some(model) => format!("{kind} ({model})"),
        None => kind,
    };
    let what = text("description")
        .or_else(|| prompt_line(input))
        .unwrap_or_else(|| ascii_line(fallback));
    clip_head(&format!("{kind}{SEPARATOR}{what}"), MAX_LABEL_CHARS)
}

fn prompt_line(input: &Value) -> Option<String> {
    let prompt = input.get("prompt")?.as_str()?;
    let line = prompt.lines().map(ascii_line).find(|l| !l.is_empty())?;
    Some(clip_head(&line, MAX_PROMPT_CHARS))
}

/// Returns `s` as one line of ASCII text, with whitespace runs collapsed.
fn ascii_line(s: &str) -> String {
    one_line(&ascii(s))
}

/// Maps Latin letters and common punctuation to ASCII and drops every other character.
fn ascii(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'à' | 'á' | 'â' | 'ã' | 'å' => out.push('a'),
            'À' | 'Á' | 'Â' | 'Ã' | 'Å' => out.push('A'),
            'ä' => out.push_str("ae"),
            'Ä' => out.push_str("Ae"),
            'è' | 'é' | 'ê' | 'ë' => out.push('e'),
            'È' | 'É' | 'Ê' | 'Ë' => out.push('E'),
            'ì' | 'í' | 'î' | 'ï' => out.push('i'),
            'Ì' | 'Í' | 'Î' | 'Ï' => out.push('I'),
            'ò' | 'ó' | 'ô' | 'õ' => out.push('o'),
            'Ò' | 'Ó' | 'Ô' | 'Õ' => out.push('O'),
            'ö' => out.push_str("oe"),
            'Ö' => out.push_str("Oe"),
            'ù' | 'ú' | 'û' => out.push('u'),
            'Ù' | 'Ú' | 'Û' => out.push('U'),
            'ü' => out.push_str("ue"),
            'Ü' => out.push_str("Ue"),
            'ç' => out.push('c'),
            'Ç' => out.push('C'),
            'ñ' => out.push('n'),
            'Ñ' => out.push('N'),
            'ý' | 'ÿ' => out.push('y'),
            'Ý' => out.push('Y'),
            'ß' => out.push_str("ss"),
            '–' | '—' => out.push('-'),
            '‘' | '’' => out.push('\''),
            '“' | '”' => out.push('"'),
            '…' => out.push_str("..."),
            c if c.is_ascii() => out.push(c),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn label_is_type_model_and_description_with_the_documented_fallbacks() {
        let cases: [(serde_json::Value, &str, String); 9] = [
            (
                json!({"subagent_type": "worker", "description": "Fix ledger torn tail",
                    "prompt": "Fix it"}),
                "Agent",
                "worker - Fix ledger torn tail".to_owned(),
            ),
            (
                json!({"description": "Map the repo"}),
                "Agent",
                "general-purpose - Map the repo".to_owned(),
            ),
            (
                json!({"subagent_type": "explore", "prompt": "\n  Find every caller of\n  parse"}),
                "Agent",
                "explore - Find every caller of".to_owned(),
            ),
            (
                json!({"prompt": "a".repeat(80)}),
                "Agent",
                format!("general-purpose - {}...", "a".repeat(57)),
            ),
            (
                json!({"subagent_type": "worker", "model": "sonnet", "description": "Fix it"}),
                "Agent",
                "worker (sonnet) - Fix it".to_owned(),
            ),
            (
                json!({}),
                "Review the diff",
                "general-purpose - Review the diff".to_owned(),
            ),
            (
                json!({"model": "sonnet", "description": "Map the repo"}),
                "Agent",
                "general-purpose (sonnet) - Map the repo".to_owned(),
            ),
            (
                json!({"subagent_type": "worker", "description": "Fix Müller – naïve 日本"}),
                "Agent",
                "worker - Fix Mueller - naive".to_owned(),
            ),
            (
                json!({"subagent_type": "wor"}),
                "Agent",
                "wor - Agent".to_owned(),
            ),
        ];
        for (input, card, want) in cases {
            assert_eq!(subagent_label(&input, card), want);
        }
        let input = json!({"subagent_type": "worker", "description": "x".repeat(200)});
        let label = subagent_label(&input, "Agent");
        assert_eq!(label.chars().count(), 100);
        assert!(label.ends_with("..."));
    }
}
