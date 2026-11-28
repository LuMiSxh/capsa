// SPDX-License-Identifier: MPL-2.0
use acus_syntax::{Lang, is_jsonc, syntax_errors};

const ZED_SETTINGS: &str = "// Zed settings\n{\n  \"theme\": \"One Dark\", // inline\n  \"tabs\": {\n    \"size\": 4,\n  },\n  /* block */\n  \"url\": \"http://example.com\",\n}\n";

#[test]
fn c_gnu_attributes_and_asm_are_not_syntax_errors() {
    let src = "#ifdef __APPLE__\n__asm__(\".zerofill A,A\");\nstatic char a[0x1000] __attribute__((section(\"A, A\")));\n#else\nstatic char b[1];\n#endif\nstatic char c[2] __attribute__((section(\"C\")));\n";
    assert_eq!(syntax_errors(Lang::C, src, false), Some(vec![]));
}

#[test]
fn c_unclosed_attribute_is_still_a_syntax_error() {
    let src = "static char a[2] __attribute__((section(\"A\"));\n";
    assert!(!syntax_errors(Lang::C, src, false).unwrap().is_empty());
}

#[test]
fn c_unclosed_block_is_still_a_syntax_error() {
    let src = "int f(void) {\n    return 1;\n";
    assert!(!syntax_errors(Lang::C, src, false).unwrap().is_empty());
}

#[test]
fn c_errors_keep_their_line_numbers_after_blanking() {
    let src = "static char a[1] __attribute__((section(\"A\")));\nint f( {\n";
    assert_eq!(
        syntax_errors(Lang::C, src, false).unwrap().first(),
        Some(&2)
    );
}

#[cfg(feature = "lang-csharp")]
#[test]
fn csharp_syntax_errors_come_from_the_c_sharp_grammar() {
    let ok = "namespace App {\n    class Repo {\n        void Load() {}\n    }\n}\n";
    assert_eq!(syntax_errors(Lang::CSharp, ok, false), Some(vec![]));
    let broken = "namespace App {\n    class Repo {\n        void Load( {\n    }\n}\n";
    assert!(
        !syntax_errors(Lang::CSharp, broken, false)
            .unwrap()
            .is_empty()
    );
}

#[cfg(feature = "lang-go")]
#[test]
fn go_syntax_errors_come_from_the_go_grammar() {
    let ok = "package main\n\nfunc Run() {}\n";
    assert_eq!(syntax_errors(Lang::Go, ok, false), Some(vec![]));
    let broken = "package main\n\nfunc Run( {\n}\n";
    assert!(!syntax_errors(Lang::Go, broken, false).unwrap().is_empty());
}

#[test]
fn jsonc_comments_and_trailing_commas_parse_in_jsonc_mode() {
    assert_eq!(syntax_errors(Lang::Json, ZED_SETTINGS, true), Some(vec![]));
    assert!(
        !syntax_errors(Lang::Json, ZED_SETTINGS, false)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn jsonc_unbalanced_brace_is_still_a_syntax_error() {
    let broken = ZED_SETTINGS.trim_end().trim_end_matches('}');
    assert!(!syntax_errors(Lang::Json, broken, true).unwrap().is_empty());
}

#[test]
fn json_is_jsonc_only_with_comments_or_trailing_commas() {
    assert!(is_jsonc(ZED_SETTINGS));
    assert!(is_jsonc("[1,]\n"));
    assert!(!is_jsonc("{\"a\": [1, 2]}\n"));
    assert!(!is_jsonc("{\"url\": \"http://example.com/*x*/\"}\n"));
}
