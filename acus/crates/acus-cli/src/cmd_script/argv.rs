// SPDX-License-Identifier: MPL-2.0
//! Turns the arguments of a script's call into the command-line words of the acus command it
//! names, so the command's own parser accepts every flag and nothing drifts.

use acus_script::HostError;
use serde_json::{Map, Value};

/// A call's arguments: the positional values and the trailing options object, if any.
pub(super) struct Call {
    pub(super) positionals: Vec<String>,
    pub(super) options: Map<String, Value>,
}

impl Call {
    /// Splits `args` into positionals and options. An array contributes its elements; an options
    /// object may only come last.
    pub(super) fn new(function: &str, args: Vec<Value>) -> Result<Self, HostError> {
        let mut call = Call {
            positionals: Vec::new(),
            options: Map::new(),
        };
        let last = args.len().saturating_sub(1);
        for (i, arg) in args.into_iter().enumerate() {
            match arg {
                Value::Object(options) if i == last => call.options = options,
                Value::Null => {}
                other => call.positionals.extend(words(function, other)?),
            }
        }
        Ok(call)
    }

    /// Returns the option `key` as a bool, which is `false` when it is absent.
    pub(super) fn flag(&self, key: &str) -> bool {
        self.options.get(key).and_then(Value::as_bool) == Some(true)
    }

    /// Returns the option `key` as a string.
    pub(super) fn text(&self, key: &str) -> Option<&str> {
        self.options.get(key).and_then(Value::as_str)
    }

    /// Returns the words of the call: the flags from the options, then `--`, then the positionals
    /// and the `paths` option. The keys in `skip` are not flags.
    pub(super) fn argv(&self, function: &str, skip: &[&str]) -> Result<Vec<String>, HostError> {
        let mut argv = Vec::new();
        for (key, value) in &self.options {
            if !skip.contains(&key.as_str()) && key != "paths" {
                flag_words(function, key, value, &mut argv)?;
            }
        }
        argv.push("--".to_owned());
        argv.extend(self.positionals.iter().cloned());
        if let Some(paths) = self.options.get("paths") {
            argv.extend(words(function, paths.clone())?);
        }
        Ok(argv)
    }
}

/// The words a positional value stands for: a string or number is one, an array is each element.
fn words(function: &str, value: Value) -> Result<Vec<String>, HostError> {
    match value {
        Value::String(s) => Ok(vec![s]),
        Value::Number(n) => Ok(vec![n.to_string()]),
        Value::Array(items) => {
            let mut all = Vec::new();
            for item in items {
                all.extend(words(function, item)?);
            }
            Ok(all)
        }
        other => Err(HostError::new(format!(
            "{function}() takes strings and arrays of strings, not {other}"
        ))
        .with_hint("pass options as the last argument, an object such as {glob: \"*.rs\"}")),
    }
}

/// Appends the flag `key` and its value. `true` is a bare flag, `false` and `null` leave it out,
/// and an array repeats it.
fn flag_words(
    function: &str,
    key: &str,
    value: &Value,
    argv: &mut Vec<String>,
) -> Result<(), HostError> {
    let name = flag_name(key);
    match value {
        Value::Null | Value::Bool(false) => {}
        Value::Bool(true) => argv.push(name),
        Value::String(s) => argv.push(with_value(&name, s)),
        Value::Number(n) => argv.push(with_value(&name, &n.to_string())),
        Value::Array(items) => {
            for item in items {
                flag_words(function, key, item, argv)?;
            }
        }
        Value::Object(_) => {
            return Err(HostError::new(format!(
                "{function}() option {key} must be a string, number, boolean or array"
            )));
        }
    }
    Ok(())
}

/// `ignoreCase` is `--ignore-case`; a one-letter key is a short flag, so `A` is `-A`.
fn flag_name(key: &str) -> String {
    if key.chars().count() == 1 {
        return format!("-{key}");
    }
    let mut name = String::from("--");
    for c in key.chars() {
        if c.is_ascii_uppercase() {
            name.push('-');
            name.push(c.to_ascii_lowercase());
        } else {
            name.push(c);
        }
    }
    name
}

/// Joins a flag and its value in one word, so a value that starts with `-` is not read as a flag.
fn with_value(name: &str, value: &str) -> String {
    if name.starts_with("--") {
        format!("{name}={value}")
    } else {
        format!("{name}{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn options_become_flags_and_positionals_follow_the_separator() {
        let cases = [
            (
                vec![
                    json!("x"),
                    json!({"block": true, "glob": "*.rs", "ignoreCase": false}),
                ],
                vec!["--block", "--glob=*.rs", "--", "x"],
            ),
            (
                vec![
                    json!(["a", "b"]),
                    json!({"A": 2, "g": ["*.rs", "!t"], "maxHits": 5}),
                ],
                vec!["-A2", "-g*.rs", "-g!t", "--max-hits=5", "--", "a", "b"],
            ),
            (
                vec![json!("a"), json!({"paths": ["src"], "x": null})],
                vec!["--", "a", "src"],
            ),
        ];
        for (args, expected) in cases {
            let call = Call::new("f", args).unwrap();
            assert_eq!(call.argv("f", &[]).unwrap(), expected);
        }
    }

    #[test]
    fn an_object_before_the_last_argument_is_refused() {
        let err = Call::new("f", vec![json!({"a": 1}), json!("x")])
            .err()
            .unwrap();
        assert!(
            err.message.starts_with("f() takes strings"),
            "{}",
            err.message
        );
    }
}
