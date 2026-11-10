// SPDX-License-Identifier: MPL-2.0
//! The `acus patch` help text that `--help` prints after the options.

/// The syntax `--help` shows after the options; `*** Replace All:` is the one agents miss.
pub(super) const SYNTAX: &str = "\
PATCH FORMAT (stdin or --file; all or nothing, nothing is written on an error):
  *** Update File: PATH         then SEARCH/REPLACE blocks (below)
  *** Add File: PATH            then `+` lines
  *** Append File: PATH         then `+` lines, added after the file's last line
  *** Delete File: PATH
  *** Copy File: SRC            then *** To: DST (later blocks may update DST)
  *** Replace Symbol: PATH#Symbol   then the new definition as `+` lines
  *** Insert Before: PATH#Symbol    then `+` lines (above its doc comments and attributes)
  *** Insert After: PATH#Symbol     then `+` lines (below the symbol)
                                    PATH:N instead of PATH#Symbol means before / after line N
  *** Delete Symbol: PATH#Symbol
  *** Move Symbol: PATH#Symbol      then *** Before: PATH#Sym, *** After: PATH#Sym or *** To: PATH
  *** Delete Lines: PATH:A-B        then *** Expect: TEXT, the exact text of line A
  *** Move Lines: PATH:A-B          then *** Expect: TEXT, *** After: PATH:N (0 = start) or PATH#Sym
                                    line numbers are from before the patch (run first)
  *** Resolve Conflict: PATH --ours    resolve every conflict section; also --theirs, --both, --base
                                    --both keeps ours then theirs; --base needs the diff3 part
  *** Resolve Conflict: PATH:LINE --theirs
                                    resolve only the section that contains line LINE
                                    line numbers are from before the patch

  SEARCH/REPLACE under `*** Update File:`, with the Edit tool's semantics: one or more blocks
  <<<<<<< SEARCH
  old text, copied exactly without the line-number prefix `acus show` prints
  =======
  new text
  >>>>>>> REPLACE
  The old text is a substring, part of a line or several lines, and must occur exactly once in
  the file. With no match the error shows the nearest lines; with several it lists their line
  numbers, so add a neighbouring line to the block to narrow the search.
  Blocks of one file and files of one patch apply all or nothing. When several blocks fail,
  one run reports every one of them, each with its nearest match; nothing is written. If the
  text is not found as written, acus also accepts it with line-number prefixes stripped,
  ignoring trailing whitespace, or ignoring leading indentation (REPLACE is then written in
  the file's indentation), and prints a `note:`. With --fuzzy a final pass ignores tabs vs
  spaces, runs of whitespace and blank lines (REPLACE is written as given) and must match once;
  it never runs unless the flag or [patch] fuzzy is set, and --no-fuzzy turns that off.
  Written files must not get more syntax errors (tree-sitter) than they had, or the patch fails
  and names the line; --allow-broken writes them anyway.

PART OF A LINE: *** Replace All: PATH [PATH...]
  Replaces every occurrence of a text in the files, directories (walked, .gitignore respected)
  and globs (*.py, matched anywhere below the current directory) named after the colon,
  separated by spaces. The text need not be a whole line and may span lines. Each block must
  match at least once, or the patch fails. Literal and regex blocks mix, and apply in order:

  *** Replace All: src *.md
  <<<<<<< SEARCH
  old_name(
  =======
  new_name(
  >>>>>>> REPLACE
  <<<<<<< REGEX
  fn (\\w+)_old\\(
  =======
  fn ${1}_new(
  >>>>>>> REPLACE

  SEARCH is literal and whitespace-exact. REGEX is a Rust regex run per file with (?m);
  `.` does not cross lines (use \\n or (?s)) and the replacement may use ${1}, ${name}.

Example (add --fmt to format the written files):
  acus patch <<'PATCH'
  *** Update File: src/lib.rs
  <<<<<<< SEARCH
      let x = 1;
  =======
      let x = 2;
  >>>>>>> REPLACE
  PATCH";
