// SPDX-License-Identifier: MPL-2.0
use super::*;

fn render(doc: &Doc, profile: Profile) -> String {
    let mut out = Vec::new();
    doc.write(&mut out, profile).unwrap();
    String::from_utf8(out).unwrap()
}

fn mark(status: Status, text: &str) -> Mark {
    Mark {
        status,
        text: text.to_owned(),
    }
}

fn census() -> Doc {
    let mut doc = Doc::default();
    doc.title("census limits");
    doc.row("5h window")
        .bar(0.48)
        .text("48%")
        .text("resets 14:20");
    doc.row("7d window")
        .bar(0.19)
        .text("19%")
        .text("resets Fri 09:00");
    doc.row("budget")
        .text("$7.12 of $100")
        .text("7%")
        .text("month ends ~$139");
    doc.marks([
        mark(Status::Ok, "hook registered"),
        mark(Status::Ok, "ledger 2 min old"),
    ]);
    doc
}

fn doctor(ledger: Status) -> Doctor {
    let checks = vec![
        Check {
            name: "hook".into(),
            status: Status::Ok,
            detail: "installed".to_owned(),
            hint: None,
        },
        Check {
            name: "ledger".into(),
            status: ledger,
            detail: "missing".to_owned(),
            hint: Some("run acus self setup".to_owned()),
        },
    ];
    Doctor::new(
        "acus",
        "0.1.0",
        vec![Group {
            name: "setup",
            checks,
        }],
    )
}

#[test]
fn census_limits_matches_the_design_example() {
    let expected = [
        "census limits\n",
        "\n",
        "  5h window     ████████░░░░░░░░  48%   resets 14:20\n",
        "  7d window     ███░░░░░░░░░░░░░  19%   resets Fri 09:00\n",
        "  budget        $7.12 of $100     7%    month ends ~$139\n",
        "\n",
        "  ✓ hook registered   ✓ ledger 2 min old\n",
    ]
    .concat();
    let plain = Profile {
        color: false,
        rich: true,
    };
    assert_eq!(render(&census(), plain), expected);
}

#[test]
fn a_non_row_block_ends_the_alignment_group() {
    let mut doc = Doc::default();
    doc.row("a_very_long_label").text("x");
    doc.line("note");
    doc.row("ab").text("y");
    let plain = Profile {
        color: false,
        rich: true,
    };
    assert_eq!(
        render(&doc, plain),
        "  a_very_long_label     x\n  note\n  ab     y\n"
    );
}

#[test]
fn a_bar_column_pads_text_to_sixteen_cells() {
    let mut doc = Doc::default();
    doc.row("a").bar(0.5).text("50%");
    doc.row("b").text("10 of 20").text("x");
    let plain = Profile {
        color: false,
        rich: true,
    };
    let expected = format!(
        "  a     {}  50%\n  b     10 of 20{}x\n",
        "████████░░░░░░░░",
        " ".repeat(10)
    );
    assert_eq!(render(&doc, plain), expected);
}

#[test]
fn the_agent_profile_is_plain_text_with_words_for_marks() {
    let mut doc = Doc::default();
    doc.title("census limits");
    doc.row("5h window")
        .bar(0.48)
        .text("48%")
        .text("resets 14:20");
    doc.marks([
        mark(Status::Ok, "hook registered"),
        mark(Status::Warn, "budget high"),
        mark(Status::Fail, "ledger missing"),
    ]);
    let text = render(&doc, Profile::AGENT);
    assert_eq!(
        text,
        "census limits\n\n5h window: 48%, resets 14:20\n\nok: hook registered\nwarn: budget high\nfail: ledger missing\n"
    );
    assert!(!text.contains('\x1b'));
}

#[test]
fn colour_appears_only_on_marks_bars_hints_and_titles() {
    let mut doc = census();
    doc.hint("run acus self setup");
    let coloured = Profile {
        color: true,
        rich: true,
    };
    let text = render(&doc, coloured);
    assert!(text.starts_with("\x1b[1mcensus limits\x1b[0m\n\n  5h window     "));
    assert!(text.contains("\x1b[32m████████\x1b[0m░░░░░░░░  48%   resets 14:20\n"));
    assert!(
        text.contains("  \x1b[32m✓\x1b[0m hook registered   \x1b[32m✓\x1b[0m ledger 2 min old\n")
    );
    assert!(text.ends_with("  \x1b[36mhint: run acus self setup\x1b[0m\n"));
    assert!(text.contains("  budget        $7.12 of $100     7%    month ends ~$139\n"));
}

#[test]
fn doctor_exits_empty_when_a_check_fails() {
    assert_eq!(doctor(Status::Fail).outcome(), Outcome::Empty);
    assert_eq!(doctor(Status::Warn).outcome(), Outcome::Found);
}

#[test]
fn doctor_json_has_the_stable_shape() {
    let mut out = Vec::new();
    let outcome = emit_to(&doctor(Status::Fail), Format::Json, false, &mut out).unwrap();
    assert_eq!(outcome, Outcome::Empty);
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "{\"tool\":\"acus\",\"version\":\"0.1.0\",\"ok\":false,\"groups\":[{\"name\":\"setup\",\"checks\":[{\"name\":\"hook\",\"status\":\"ok\",\"detail\":\"installed\"},{\"name\":\"ledger\",\"status\":\"fail\",\"detail\":\"missing\",\"hint\":\"run acus self setup\"}]}]}\n"
    );
}

#[test]
fn doctor_human_view_marks_checks_and_indents_the_hint() {
    let plain = Profile {
        color: false,
        rich: true,
    };
    let expected = format!(
        "acus self doctor\n\nsetup\n  ✓ hook{}installed\n  ✗ ledger{}missing\n    hint: run acus self setup\n",
        " ".repeat(7),
        " ".repeat(5)
    );
    let mut doc = Doc::default();
    doctor(Status::Fail).doc(&mut doc);
    assert_eq!(render(&doc, plain), expected);
}

#[test]
fn doctor_agent_view_uses_mark_words() {
    let mut out = Vec::new();
    emit_to(&doctor(Status::Fail), Format::Agent, false, &mut out).unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "acus self doctor\n\nsetup\nok: hook: installed\nfail: ledger: missing\nhint: run acus self setup\n"
    );
}

#[test]
fn table_right_aligns_numeric_columns() {
    let mut doc = Doc::default();
    doc.table(Table {
        head: vec!["name".to_owned(), "count".to_owned()],
        align: vec![Align::Left, Align::Right],
        rows: vec![
            vec!["alpha".to_owned(), "7".to_owned()],
            vec!["beta".to_owned(), "1234".to_owned()],
        ],
    });
    let plain = Profile {
        color: false,
        rich: true,
    };
    let expected = format!(
        "  name{}count\n  alpha{}7\n  beta{}1234\n",
        " ".repeat(3),
        " ".repeat(6),
        " ".repeat(4)
    );
    assert_eq!(render(&doc, plain), expected);
}
