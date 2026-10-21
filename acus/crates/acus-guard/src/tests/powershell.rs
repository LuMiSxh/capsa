// SPDX-License-Identifier: MPL-2.0
//! Guard tests for PowerShell commands and Windows paths.

use super::*;

#[test]
fn recognises_windows_commands_in_either_shell() {
    for (cmd, rule) in [
        ("grep.exe -rn foo src", "grep"),
        (
            "'C:\\Program Files\\Git\\usr\\bin\\grep.exe' -n x a.rs",
            "grep",
        ),
        (r"C:\tools\rg.exe foo", "grep"),
        ("findstr /s /i foo *.rs", "grep"),
        (r#"findstr /n /c:"two words" src\a.rs"#, "grep"),
        (r"type src\a.rs", "read-shell"),
        (r"more src\a.rs", "read-shell"),
        (r"cd src && type a.rs b.rs", "read-shell"),
    ] {
        let Some(Verdict::Deny(r, _)) = check(cmd) else {
            panic!("{cmd}");
        };
        assert_eq!(r.as_str(), rule, "{cmd}");
    }
    for cmd in [
        "findstr /i error build.log",
        r#"findstr /c:"a b" out.txt"#,
        r"type notes.txt",
        r"type package.json",
        "type a.rs > b.rs",
    ] {
        assert_eq!(check(cmd), None, "{cmd}");
    }
    assert!(matches!(
        check("dir /s"),
        Some(Verdict::Hint(Rule::Tree, _))
    ));
    let (_, r) = deny(check(r#"cd x && findstr /n /c:"two words" src\a.rs"#));
    assert!(r.contains(r"`acus find 'two words' 'src\a.rs' -F`"), "{r}");
}

#[test]
fn covers_powershell_habits() {
    for (cmd, rule) in [
        (r"gc -Raw src\a.rs", "read-shell"),
        (r"Get-Content -TotalCount 5 .\src\a.rs", "read-shell"),
        (r"Get-Content -Path 'src\a.rs' -Tail 5", "read-shell"),
        ("Get-Content a.rs,b.txt", "read-shell"),
        (r"cat -Raw a.rs", "read-shell"),
        (r"type C:\repo\SRC\MAIN.RS", "read-shell"),
        (r"sls foo src\*.rs", "grep"),
        (r"Select-String -Pattern 'a b' -Path a.rs,b.rs", "grep"),
        (r"& 'C:\tools\rg.exe' foo src", "grep"),
        (r"findstr /s foo *.swift", "grep"),
        (r"gci -Recurse -Filter *.rs | Select-String foo", "grep"),
        (r"Get-ChildItem -Recurse | sls foo", "grep"),
        (r"ls -r src | Select-String -Pattern foo", "grep"),
        (r"dir *.rs | sls foo", "grep"),
        (
            r"(Get-Content a.rs) -creplace 'x','y' | sc a.rs",
            "ps-replace",
        ),
        (
            r"(gc a.rs -Raw).Replace('x','y') | Out-File a.rs",
            "ps-replace",
        ),
        (r"(Get-Content a.rs) -replace 'x','y' > a.rs", "ps-replace"),
        (
            r"[IO.File]::WriteAllText('a.rs', [IO.File]::ReadAllText('a.rs').Replace('x','y'))",
            "ps-replace",
        ),
        (r#"[System.IO.File]::ReadAllText("src\a.rs")"#, "read-shell"),
        ("cargo build 2>&1 | select -First 20", "build-pipe"),
    ] {
        let Some(Verdict::Deny(r, reason)) = check_ps(cmd, None) else {
            panic!("{cmd}");
        };
        assert_eq!(r.as_str(), rule, "{cmd}");
        assert!(reason.contains("# acus-skip"), "{cmd}: {reason}");
    }
    for cmd in [
        "Get-Content app.log -Tail 20 -Wait",
        "Get-Content settings.json -Raw",
        "Get-Content out.txt | Select-String error",
        "Select-String error build.log",
        "Get-ChildItem -Recurse -Include *.log | Select-String error",
        "gci *.txt | sls foo",
        "gci -Name | sls foo",
        "ls | Select-Object -First 3",
        "Write-Host 'cat a.rs; sls x a.rs'",
        "Set-Content notes.txt 'hello'",
        r"(Get-Content notes.txt) -replace 'x','y' | Set-Content notes.txt",
        r"[IO.File]::ReadAllText('data.json')",
    ] {
        assert_eq!(check_ps(cmd, None), None, "{cmd}");
    }
    for cmd in [
        "ls -r src",
        "gci -Rec",
        "Get-ChildItem -Recurse -Depth 2",
        "dir /s",
    ] {
        assert!(
            matches!(check_ps(cmd, None), Some(Verdict::Hint(Rule::Tree, _))),
            "{cmd}"
        );
    }
    // `ls -r` reverses in Bash.
    assert_eq!(check("ls -r src"), None);
}

#[test]
fn powershell_separators_and_quoting_split_commands_the_same() {
    // `;` everywhere, `&&` and `||` (PowerShell 7; Windows PowerShell 5.1 rejects them but
    // the guard still sees the parts), backslashes are paths, quotes hide separators.
    for cmd in [
        r"cd src; Select-String foo a.rs",
        r"cd src && Select-String foo a.rs",
        r"cd src || Select-String foo a.rs",
        r"ls src\; Select-String foo src\a.rs",
        "Write-Host \"a`\";\" ; Get-Content a.rs",
        "Write-Host 'it''s'; Get-Content a.rs",
    ] {
        assert!(
            matches!(check_ps(cmd, None), Some(Verdict::Deny(..))),
            "{cmd}"
        );
    }
    let (_, r) = deny(check_ps(
        r"cd C:\Repo; Get-Content Src\Main.rs; Get-Date",
        None,
    ));
    assert!(r.contains(r"because of `Get-Content Src\Main.rs`"), "{r}");
    assert!(r.contains(r"`acus show 'Src\Main.rs'`"), "{r}");
    // The escape works on a refused segment of a compound command.
    assert_eq!(
        check_ps(r"cd src; Get-Content a.rs # acus-skip", None),
        Some(Verdict::Escape(Rule::ReadShell))
    );
}

#[test]
fn names_the_ctx_flag_for_powershell_filters() {
    for (cmd, want) in [
        (
            "cargo build 2>&1 | Select-Object -Last 30",
            "`acus ctx 'cargo build' --tail 30`",
        ),
        (
            "cargo build | select -First 20",
            "`acus ctx 'cargo build' --max-output 20`",
        ),
        (
            "cargo test | Select-String Error",
            "`acus ctx 'cargo test' --grep '(?i)Error'`",
        ),
        (
            "cargo test | sls -Pattern 'FAILED|panicked'",
            "`acus ctx 'cargo test' --grep '(?i)FAILED|panicked'`",
        ),
        ("acus ctx | Select-Object -Last 5", "`acus ctx --tail 5`"),
    ] {
        let (_, r) = deny(check_ps(cmd, None));
        assert!(
            r.contains(&format!("Use {want} instead of piping")),
            "{cmd}: {r}"
        );
        assert!(r.contains("# acus-skip"), "{cmd}: {r}");
    }
}

#[test]
fn drive_paths_are_read_paths_in_powershell() {
    for (cmd, want) in [
        (
            r"cd C:\work; Get-Content C:\work\a.rs -TotalCount 5",
            r"acus show 'C:\work\a.rs:1-5'",
        ),
        (
            "cd C:/work; Get-Content C:/work/a.rs -TotalCount 5",
            "acus show 'C:/work/a.rs:1-5'",
        ),
        (
            r"cd C:\work; type C:\work\a.rs",
            r"acus show 'C:\work\a.rs'",
        ),
    ] {
        let (rule, reason) = deny(check_ps(cmd, None));
        assert_eq!(rule, "read-shell", "{cmd}");
        assert!(reason.contains(&format!("`{want}`")), "{cmd}: {reason}");
    }
}

#[test]
fn applies_the_rules_to_powershell() {
    for (cmd, rule) in [
        ("Select-String -Path src\\*.rs -Pattern foo", "grep"),
        ("Get-Content src\\main.rs", "read-shell"),
        ("cargo test 2>&1 | Select-Object -Last 30", "build-pipe"),
        (
            "(Get-Content a.rs) -replace 'x','y' | Set-Content a.rs",
            "ps-replace",
        ),
    ] {
        match check_ps(cmd, None) {
            Some(Verdict::Deny(r, reason)) => {
                assert_eq!(r.as_str(), rule, "{cmd}");
                assert!(reason.contains("# acus-skip"), "{reason}");
            }
            other => panic!("{cmd}: {other:?}"),
        }
    }
    assert_eq!(
        check_ps("Get-Content src\\main.rs # acus-skip", None),
        Some(Verdict::Escape(Rule::ReadShell))
    );
    for cmd in [
        "git log --oneline | Select-String fix",
        "Get-Content settings.ini",
        "Get-ChildItem -Recurse *.rs",
    ] {
        assert_eq!(check_ps(cmd, None), None, "{cmd}");
    }
}

#[cfg(windows)]
#[test]
fn shows_windows_paths_relative_to_the_session() {
    assert_eq!(
        super::relative(r"C:\repo\src\a.rs", Some(r"C:\repo")),
        r"src\a.rs"
    );
    assert_eq!(super::relative(r"C:\repo", Some(r"C:\repo")), ".");
    assert_eq!(
        super::relative(r"D:\other\a.rs", Some(r"C:\repo")),
        r"D:\other\a.rs"
    );
}
