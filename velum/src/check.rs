// SPDX-License-Identifier: MPL-2.0
//! Owns check results, rendered as agent lines, a human table or JSON.

use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::{Format, color_stdout};
use serde_json::{Value, json};
use std::io::{self, Write};

/// Records how a check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    /// Returns the word every output format uses.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }

    /// Returns the colour of the status in the human table.
    const fn colour(self) -> &'static str {
        match self {
            Self::Ok => "\x1b[32m",
            Self::Warn => "\x1b[33m",
            Self::Fail => "\x1b[31m",
        }
    }
}

/// Records one check and what it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
}

impl Check {
    pub fn new(name: &'static str, status: Status, detail: impl Into<String>) -> Self {
        Self {
            name,
            status,
            detail: detail.into(),
        }
    }
}

/// Returns exit 1 when a check failed, 0 otherwise. A warning is not a failure.
pub fn outcome(checks: &[Check]) -> Outcome {
    if checks.iter().any(|check| check.status == Status::Fail) {
        Outcome::Code(1)
    } else {
        Outcome::Found
    }
}

/// Prints the checks to stdout in `format` and returns their outcome.
pub fn report(format: Format, checks: &[Check]) -> Result<Outcome> {
    render(
        &mut io::stdout().lock(),
        format,
        checks,
        color_stdout(format),
    )?;
    Ok(outcome(checks))
}

/// Writes the checks to `out`. Colour is used only in the human table.
pub fn render(
    out: &mut impl Write,
    format: Format,
    checks: &[Check],
    color: bool,
) -> io::Result<()> {
    match format {
        Format::Agent => checks.iter().try_for_each(|check| {
            writeln!(
                out,
                "{} {}: {}",
                check.status.as_str(),
                check.name,
                check.detail
            )
        }),
        Format::Json => {
            let items: Vec<Value> = checks
                .iter()
                .map(|check| {
                    json!({"check": check.name, "status": check.status.as_str(), "detail": check.detail})
                })
                .collect();
            writeln!(out, "{}", Value::Array(items))
        }
        Format::Human => render_table(out, checks, color),
    }
}

fn render_table(out: &mut impl Write, checks: &[Check], color: bool) -> io::Result<()> {
    let width = checks
        .iter()
        .map(|check| check.name.len())
        .max()
        .unwrap_or(0)
        .max("CHECK".len());
    writeln!(out, "{:<7}{:<width$}  DETAIL", "STATUS", "CHECK")?;
    for check in checks {
        let label = format!("{:<7}", check.status.as_str());
        let label = if color {
            format!("{}{label}\x1b[0m", check.status.colour())
        } else {
            label
        };
        writeln!(out, "{label}{:<width$}  {}", check.name, check.detail)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Check> {
        vec![
            Check::new("srt", Status::Ok, "/bin/srt 0.0.81"),
            Check::new("claude", Status::Fail, "not found on PATH"),
        ]
    }

    fn text(format: Format, color: bool) -> String {
        let mut out = Vec::new();
        render(&mut out, format, &sample(), color).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn agent_lines_read_status_check_detail() {
        assert_eq!(
            text(Format::Agent, false),
            "ok srt: /bin/srt 0.0.81\nfail claude: not found on PATH\n"
        );
    }

    #[test]
    fn json_is_an_array_of_checks() {
        let value: Value = serde_json::from_str(&text(Format::Json, false)).unwrap();
        assert_eq!(
            value[0],
            json!({"check": "srt", "status": "ok", "detail": "/bin/srt 0.0.81"})
        );
        assert_eq!(value[1]["status"], "fail");
    }

    #[test]
    fn human_table_aligns_and_colours_only_when_asked() {
        assert_eq!(
            text(Format::Human, false),
            "STATUS CHECK   DETAIL\nok     srt     /bin/srt 0.0.81\nfail   claude  not found on PATH\n"
        );
        let coloured = text(Format::Human, true);
        assert!(coloured.contains("\x1b[32mok     \x1b[0m"));
        assert!(coloured.contains("\x1b[31mfail   \x1b[0m"));
    }

    #[test]
    fn a_failed_check_makes_the_outcome_one() {
        assert_eq!(outcome(&sample()), Outcome::Code(1));
        assert_eq!(outcome(&sample()[..1]), Outcome::Found);
    }
}
