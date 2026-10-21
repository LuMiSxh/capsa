// SPDX-License-Identifier: MPL-2.0
//! Guard tests for which backend reads a line, and for the zsh and PowerShell backends.

use super::*;

#[cfg(feature = "zsh")]
#[test]
fn the_zsh_grammar_reads_zsh_syntax_and_still_rejects_broken_input() {
    let zsh = |cmd: &str| parse_with(cmd, Dialect::Bash, Backend::Zsh);
    for cmd in [
        "print -rl -- ${(f)\"$(cat a.md)\"}",
        "diff =(cat a.md) b.md",
        "ls *(.)",
        "[[ -f a.md && $x == b* ]] && cat a.md",
    ] {
        assert!(zsh(cmd).is_some(), "{cmd}");
    }
    assert!(zsh("cat a.md | grep 'x").is_none());
    // The grammar's scanner aborts the process on deep nesting, so deep input never reaches it.
    let deep =
        |open: &str, close: &str| format!("{}cat a.md{}", open.repeat(400), close.repeat(400));
    assert!(zsh(&deep("echo \"$(", ")\"")).is_none());
    assert!(zsh(&deep("{ ", "; }")).is_none());
    assert!(zsh(&deep("(", ")")).is_none());
    // The commands inside zsh-only constructs are segments, and a zsh loop is a loop.
    let s = zsh("x=${(f)\"$(cat a.md)\"}; repeat 3 { cat b.md }")
        .unwrap()
        .segs;
    // The substitution's command comes last.
    let [body, sub] = &s[..] else { panic!() };
    assert_eq!((body.low.as_str(), body.nest), ("cat b.md", Nest::Loop));
    assert_eq!((sub.low.as_str(), sub.nest), ("cat a.md", Nest::Top));
    assert_eq!(sub.out, Out::Capture);
    let s = zsh("diff =(cat a.md) b.md").unwrap().segs;
    assert_eq!(s[0].low, "diff = b.md");
    assert_eq!(s[1].out, Out::Capture);
}

#[test]
fn the_backend_follows_the_tool_the_shell_and_the_grammars_built_in() {
    let all = Grammars {
        bash: true,
        zsh: true,
        powershell: true,
    };
    let none = Grammars {
        bash: false,
        zsh: false,
        powershell: false,
    };
    let pick = |d, shell, have| choose(d, shell, have);
    // Under zsh the zsh grammar reads Bash-tool commands; under any other shell, or an unknown
    // one, Bash.
    assert_eq!(pick(Dialect::Bash, Some("zsh"), all), Backend::Zsh);
    assert_eq!(pick(Dialect::Bash, Some("bash"), all), Backend::Bash);
    assert_eq!(pick(Dialect::Bash, Some("fish"), all), Backend::Bash);
    assert_eq!(pick(Dialect::Bash, None, all), Backend::Bash);
    // Without the zsh grammar zsh falls back to Bash, and without Bash a bash shell gets zsh.
    let no_zsh = Grammars { zsh: false, ..all };
    assert_eq!(pick(Dialect::Bash, Some("zsh"), no_zsh), Backend::Bash);
    let no_bash = Grammars { bash: false, ..all };
    assert_eq!(pick(Dialect::Bash, Some("bash"), no_bash), Backend::Zsh);
    assert_eq!(pick(Dialect::Bash, None, none), Backend::Legacy);
    assert_eq!(pick(Dialect::Bash, Some("zsh"), none), Backend::Legacy);
    // PowerShell ignores the shell.
    assert_eq!(
        pick(Dialect::PowerShell, Some("zsh"), all),
        Backend::PowerShell
    );
    assert_eq!(pick(Dialect::PowerShell, None, no_zsh), Backend::PowerShell);
    let no_ps = Grammars {
        powershell: false,
        ..all
    };
    assert_eq!(pick(Dialect::PowerShell, None, no_ps), Backend::Legacy);
    // `$SHELL` is a path.
    assert_eq!(shell_name("/bin/zsh"), "zsh");
    assert_eq!(shell_name(r"C:\msys64\usr\bin\zsh.exe"), "zsh");
    assert_eq!(shell_name("zsh"), "zsh");
}

#[test]
fn tests_ignore_the_shell_of_whoever_runs_them() {
    // The default is no shell, so the golden snapshots do not depend on `$SHELL`.
    assert_eq!(current_shell(), None);
    tests_shell::with(Some("zsh"), || {
        assert_eq!(current_shell().as_deref(), Some("zsh"));
    });
    assert_eq!(current_shell(), None);
}

#[cfg(feature = "powershell")]
#[test]
fn powershell_segments_know_their_output_nesting_and_gates() {
    let ps = |cmd: &str| parse(cmd, Dialect::PowerShell).expect("parses").segs;
    let s = ps("cd src && Get-Content a.rs | Select-String foo > out.txt; gc b");
    let [cd, gc, sls, last] = &s[..] else {
        panic!()
    };
    assert_eq!(
        (cd.low.as_str(), cd.gates, cd.out),
        ("cd src", true, Out::Agent)
    );
    assert_eq!((gc.stdin, gc.out, gc.gates), (false, Out::Pipe, false));
    assert_eq!((sls.stdin, sls.out), (true, Out::File));
    assert_eq!(
        (last.low.as_str(), last.out, last.gates),
        ("gc b", Out::Agent, false)
    );
    // Cmdlets ignore case: the rules read `low`, a refusal quotes `orig`.
    assert_eq!(
        (gc.low.as_str(), gc.orig.as_str()),
        ("get-content a.rs", "Get-Content a.rs")
    );

    // `2>err.txt` and `>$null` are not stdout redirects of the grammar's making, nor `2>&1`.
    let s =
        ps("gc a 2>&1; gc b 2>err.txt; gc c >$null; gc d *> all.txt; Get-Content e | Out-File f");
    assert_eq!(
        s.iter().map(|s| s.out).collect::<Vec<_>>(),
        [
            Out::Agent,
            Out::Agent,
            Out::File,
            Out::File,
            Out::Pipe,
            Out::File
        ]
    );

    let s = ps("foreach ($f in $files) { gc $f }; if ($x) { gc a }; & { gc b }");
    assert_eq!(
        s.iter().map(|s| s.nest).collect::<Vec<_>>(),
        // `& { … }` is a command (`{}`) around the group.
        [Nest::Loop, Nest::Group, Nest::Top, Nest::Group]
    );
    let s = ps("while ($true) { if ($x) { gc a } }; function F { gc a }");
    assert_eq!((s[0].nest, s[0].out), (Nest::Loop, Out::Agent));
    assert_eq!((s[1].nest, s[1].out), (Nest::Group, Out::Capture));

    // Script blocks passed as arguments and `( )`, `$( )`, `@( )` and assignments are data.
    let s = ps(
        "gci | ForEach-Object { gc $_ } | sc a.rs; $x = gc b; Write-Host $(gc c) (gc d) @(gc e)",
    );
    let lows: Vec<_> = s.iter().map(|s| s.low.as_str()).collect();
    assert_eq!(
        lows,
        [
            "gci",
            "foreach-object {}",
            "sc a.rs",
            "write-host $() () @()",
            "gc $_",
            "gc b",
            "gc c",
            "gc d",
            "gc e"
        ]
    );
    assert!(s[4..].iter().all(|s| s.out == Out::Capture));
    assert_eq!((s[1].stdin, s[1].out), (true, Out::Pipe));
    // A call operator is not part of the command the rules read.
    assert_eq!(
        ps("& 'C:\\tools\\rg.exe' foo src")[0].low,
        "'c:\\tools\\rg.exe' foo src"
    );
    // A line the grammar rejects is read by the splitter, as before the grammar.
    assert_eq!(ps("Get-Content 'a.rs").len(), 1);
}
