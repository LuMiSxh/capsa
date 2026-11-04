# patch

`acus patch --file F` reads a patch from a file instead of stdin and applies it all or nothing.
SEARCH blocks have the Edit tool's semantics and need no prior read. Put every change of one step
in one patch.

## Contents

- [Update File](#update-file)
- [Add, Append, Delete, Copy](#add-append-delete-copy)
- [Symbols](#symbols)
- [Replace All](#replace-all)
- [Line directives](#line-directives)
- [Conflicts](#conflicts)
- [Output and checks](#output-and-checks)

## Update File

```bash
acus patch <<'PATCH'
*** Begin Patch
*** Update File: src/lib.rs
<<<<<<< SEARCH
        let x = 1;
=======
        let x = 2;
>>>>>>> REPLACE
*** End Patch
PATCH
```

- Copy the old text exactly, without the line-number prefix `acus show` prints. It is a
  substring, part of a line or several lines, and must occur exactly once. On no match the error
  shows the nearest lines; on several it lists their line numbers, so add a neighbouring line.
- When the text is not found as written, a form with line-number prefixes stripped, trailing
  whitespace ignored or leading indentation ignored (tabs against spaces) is accepted with a
  `note:`; REPLACE then takes the file's indentation.
- No `+` prefixes SEARCH or REPLACE text. `+` prefixes only Replace Symbol, Insert Before/After
  and Add File bodies.
- A multi-line SEARCH matches a CRLF file as LF text with a `note:`; the file stays CRLF.
- A patch that gives a file more tree-sitter syntax errors than it had fails and names the line.
  `--allow-broken` writes it anyway.
- The patch is all or nothing. Every failing block is reported in one run with its nearest
  match; fix them all and resend once. `--check` validates a large patch without writing.

## Add, Append, Delete, Copy

```
*** Add File: path/new.rs
+line one
+line two
*** Append File: path/log.md
+appended line
*** Delete File: path/old.rs
*** Copy File: path/src.rs
*** To: path/copy.rs
```

- `*** Add File` refuses an existing file; use `*** Update File`, `*** Append File` or
  `*** Replace All`.
- `*** Append File` adds the `+` lines after the last line, with one newline between; the file
  must exist.
- `*** Copy File` copies, then `*** To: DST`; later blocks may update DST.

## Symbols

```
*** Replace Symbol: src/a.rs#Parser::reset
+    fn reset(&mut self) {
+        self.pos = 0;
+    }
*** Insert Before: src/a.rs#Parser::reset
+fn helper() {}
*** Insert After: src/a.rs#Parser::reset
+fn after() {}
*** Delete Symbol: src/a.rs#unused
*** Move Symbol: src/a.rs#helper
*** After: src/b.rs#main
```

- A symbol may be written `Class.method`; a unique suffix of the name is enough.
- Replace Symbol, Insert and Delete take doc comments and attributes along; Insert Before goes
  above them, Insert After below the symbol.
- `Move Symbol` needs `*** Before: PATH#Sym`, `*** After: PATH#Sym` or `*** To: PATH`.
- `PATH:N` instead of `PATH#Sym` means before or after line N.

## Replace All

```
*** Replace All: src *.md
<<<<<<< SEARCH
old_name(
=======
new_name(
>>>>>>> REPLACE
<<<<<<< REGEX
fn (\w+)_old\(
=======
fn ${1}_new(
>>>>>>> REPLACE
```

- The paths are space-separated files, directories and globs (`*.py`, matched below the current
  directory); every occurrence in them changes.
- SEARCH is literal and whitespace-exact; it may span lines. REGEX is a Rust regex run per file
  with `(?m)`, with `${1}` and `${name}` in the replacement; `.` does not cross lines (use `\n`
  or `(?s)`).
- A block that matches nothing fails the patch.

## Line directives

```
*** Delete Lines: src/a.rs:40-44
*** Expect:     let old = 1;
*** Move Lines: src/a.rs:40-44
*** Expect:     let old = 1;
*** After: src/a.rs#Parser::reset
```

- A line directive needs `*** Expect:` on the next line with the exact text of line A, or it
  fails and shows the actual line.
- `*** After: PATH:N` (0 is the file start) or `*** After: PATH#Sym` places moved lines.
- Numbers refer to the file before the patch, so line directives run first. On a file that also
  has `*** Resolve Conflict`, they still refer to the file before the patch, and a range inside
  a section being resolved fails.
- Overlapping ranges, a target inside the moved lines and ranges past the end are refused.

## Conflicts

```
*** Resolve Conflict: src/app.py --ours
*** Resolve Conflict: src/app.py:12 --theirs
```

- `--ours`, `--theirs`, `--both` or `--base` resolve every conflict section; `--both` keeps ours
  then theirs, and `--base` needs the diff3 part.
- `PATH:LINE --theirs` resolves only the section containing LINE, so one patch can pick different
  sides. Line numbers are from before the patch.
- A file that still has conflict markers is syntax-checked only on its result.

## Output and checks

- The written lines and their new numbers print by default; no re-read is needed.
  `acus patch -q` prints only the summary and errors when those lines are not needed.
- `*** Add File` prints only its summary. A Replace Symbol body over 40 lines prints its first
  and last 5 lines with a `… N more lines` marker; `acus patch --json` keeps every line.
- `acus patch --check` validates and reports what would change; `acus patch --diff` prints a
  unified diff, and `acus patch --check --diff` previews without writing.
- `acus patch --fmt` runs the file's formatter (rustfmt, ruff/black, prettier, swift-format,
  gofmt) over the whole file and prints the formatted lines. It also changes lines you did not
  edit, so add it only where the project requires formatted code. `acus patch --no-fmt` skips a
  configured `[patch] fmt`.
- `acus patch --allow-broken` writes a file that gains syntax errors.
- Pick a heredoc delimiter that no line of the patch equals (`PATCH`, or `ACUS_END` when editing
  docs or scripts).
