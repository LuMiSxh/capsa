# script

## Contents

- [Overview](#overview)
- [Cookbook](#cookbook)
- [Surface](#surface)
- [Results](#results)
- [Errors](#errors)
- [Parallel calls and memo](#parallel-calls-and-memo)
- [Output](#output)
- [Permissions](#permissions)
- [Saved scripts](#saved-scripts)
- [MCP servers](#mcp-servers)
- [Batch](#batch)
- [Common mistakes](#common-mistakes)

## Overview

```bash
acus script <<'JS'
const hits = await find('fn parse', {paths: ['src'], glob: '*.rs'});
print(hits.paths());
return await show(hits.head(2).map((h) => `${h.path}:${h.line}-${h.line + 20}`));
JS
acus script -e 'return await run("cargo test", {tail: 30})'
acus script --check -e 'return await run("cargo test", {tail: 30})'   # parse only
```

The program is the body of an async function: `await` and `return` work at the top level. Use one
script for 2+ calls you can plan before seeing results, even when a later call takes an earlier
result: `await` it in the code. A step that needs your judgement stays a separate call. The
script's `read`, `write`, `edit` and `fetch` replace Claude's Read, Write, Edit and WebFetch tools,
under the same permission rules. `--check` parses the program and stops; nothing runs and nothing
changes, so a typo costs no turn.

## Cookbook

Each recipe is one call. Change the paths and patterns; keep the `await`.

```js
// Find, then read the matching symbol: one call instead of find, three reads.
const hits = await find("fn parse", { block: true, maxHits: 20 });
return await show(hits.head(3).map((h) => `${h.path}:${h.start}-${h.end}`));

// Map a repo before editing: files with sizes, then the outline of the touched crate.
const files = await files(["src", "tests"], { glob: "*.rs", lines: true });
const outline = await outline("src/lib.rs");
return { files: files.length, symbols: outline[0]?.symbols.length };

// Edit and check in one call; a failing check keeps the check output.
await edit("src/lib.rs", "fn old() {}", "fn new() {}");
return await run("cargo test", { tail: 40 });

// Run a check and keep its failure block, or return the clean result.
const test = await run("cargo test -p app", { tail: 80 });
return test;

// Read structured data: JSON and CSV come back as values, not text.
const manifest = await readJson("package.json");
const rows = parse.csv(await read("metrics.csv"));
return { name: manifest.name, failing: rows.filter((r) => Number(r.errors) > 0) };
```

- A recipe that keeps its use belongs in a saved script: `acus script save NAME` (see Saved
  scripts); the next session starts with the names it can run.
- `readJson` obeys the same Read rules as `read`. `parse.csv` takes `{delimiter, header}`;
  `header` defaults to `true`, and a missing trailing field is `null`.

## Surface

The shapes below are TypeScript-style for reading; the program is JavaScript.

```ts
// Every function returns a Promise. The options object is the last argument: a key is the
// command's long flag in camelCase (`ignoreCase` is --ignore-case), a one-letter key is a short
// flag, `true` is a bare flag and an array repeats the flag. `paths` adds positional paths.
// `acus help <command>` lists the flags.

type Hit = { path: string; line: number; symbol?: string; text: string };
type View = { path: string; start: number; end: number; symbol?: string; text: string };
type FileHit = { path: string; count?: number };
type Outline = {
  path: string; lang?: string; lines?: number;
  symbols: { kind: string; name: string; qual: string; depth: number; start: number; end: number; detail?: string }[];
};
type DiffFile = {
  path: string; status: "A" | "D" | "R" | "M" | "U" | "?"; add: number; del: number;
  symbols: { symbol: string; start?: number; end?: number; add: number; del: number; lines: { n: number; added: unknown; text: string }[] }[];
};
type Run = { kind: "run"; ok: boolean; code: number; text: string; failures?: unknown[]; json?: unknown };
type Patch = {
  kind: "patch"; check: boolean; written: unknown; changes: unknown; notes: unknown;
  regions: { path: string; start: number; lines: number }[]; diffs: unknown; run?: Run;
};

// Lists carry `kind` and `text`; .head(n), .tail(n), .grep(re) and .paths() keep them.
find(pattern: string | string[], o?: { paths?: string[]; glob?: string; block?: boolean; filesWithMatches?: boolean; count?: boolean }): Promise<Hit[] | View[] | FileHit[]>;   // block: views; filesWithMatches or count: files
show(...addresses: string[]): Promise<View[]>;
outline(paths: string | string[]): Promise<Outline[]>;
files(paths?: string | string[], o?: { glob?: string; lines?: boolean }): Promise<{ path: string; lines?: number }[]>;
follow(address: string, o?: { down?: boolean }): Promise<{ from: string; to: string; lines: unknown; text: string; sure: boolean; depth: number }[]>;
blame(address: string): Promise<{ start: number; end: number; commit: string; author: string; date: string; summary: string }[]>;
diff(rev?: string, o?: { patch?: boolean; paths?: string[] }): Promise<DiffFile[]>;
log(o?: { n?: number }): Promise<{ hash: string; date: string; author: string; subject: string; files: number; add: number; del: number }[]>;

// Writes wait for the calls before them and hold back the calls after them.
patch(text: string, o?: { check?: boolean; diff?: boolean; run?: string }): Promise<Patch>;
patch(edits: { path: string; old: string; new: string; all?: boolean }[]): Promise<Patch>;   // all or nothing
write(path: string, text: string): Promise<Patch>;     // creates or replaces a file under the repository; writes the text byte for byte, no newline added
edit(path: string, old: string, new: string, o?: { all?: boolean }): Promise<Patch>;   // old occurs once unless all

run(cmd: string, o?: { tail?: number; grep?: string; drop?: string | string[]; timeout?: number; json?: boolean }): Promise<Run>;
read(path: string, o?: { offset?: number; limit?: number; numbered?: boolean }): Promise<string | { kind: "image" | "binary"; path: string; bytes: number; hint: string }>;
readJson(path: string): Promise<unknown>;   // parses a JSON file under the same Read rules
stat(path: string): Promise<{ exists: boolean; dir: boolean; bytes?: number; modified?: string; lines?: number }>;   // modified: RFC 3339 time
exists(path: string): Promise<boolean>;
env(name: string): Promise<string | null>;   // "<set>" for names with KEY, TOKEN, SECRET, PASS, AUTH or CREDENTIAL
fetch(url: string, o?: { grep?: string; head?: number }): Promise<{ status: number; url: string; text: string }>;
textDiff(old: string, new: string, o?: { context?: number }): Promise<string>;   // unified diff; context defaults to 3
parse.toml(text: string): Promise<unknown>;   // datetimes as RFC 3339 text
parse.yaml(text: string): Promise<unknown>;   // the first document only; a later one is ignored
parse.csv(text: string, o?: { delimiter?: string; header?: boolean }): Promise<Record<string, string>[] | string[][]>;   // header defaults to true

sleep(ms: number): Promise<null>;   // stops at the script's deadline
retry<T>(fn: () => Promise<T>, o?: { times?: number; delayMs?: number }): Promise<T>;   // defaults 3, 200
all<T>(calls: Promise<T>[]): Promise<(T | { ok: false; error: string; hint: string | null; call: string | null })[]>;
print(...values: unknown[]): void;
// import gh from "mcp:NAME"; `tools` lists [{name, description}]; each tool is an async method.
```

## Results

- `print` values come first, then the result. Strings print as they are, command results as the
  text of their command, anything else as compact JSON.
- Without `return` or `print` anywhere in the program, its last expression is the result. Naming
  either word turns that off.
- A `hits`, `views`, `files`, `outline`, `follow`, `diff`, `log`, `blame` or `run` result prints as
  its command would. A plain array or object prints as compact JSON: print each result to get text.
- `--json` prints `{prints, unawaited, result}` with the objects whole and is not capped.

## Errors

- A failed call throws an `Error` with `.message`, `.hint`, `.call` and `.args`. Catch it, or let it
  end the script: the output then names the program line, `error: line N: ...`, with its `hint:`.
- `all([...])` turns each failure into `{ok: false, error, hint, call}` and keeps the other results.
- A failed script still prints what it printed and the host calls that finished unawaited, then
  exits 2 with the error. `--json` lists those calls under `unawaited`.
- Exit: 0 a result, 1 an empty one, 2 an error.
- A `run()` refused by the guard or a `Bash(...)` rule throws with the rule or the guard's reason as
  its hint. Use `read`, `show` or `find` for files; `show('f:1-20')` replaces `run('cat f | head')`.

## Parallel calls and memo

Every function returns a promise and starts at once, so `await Promise.all([find(a), find(b),
run('cargo test')])` runs them together, up to 8 at a time. Calls you do not await still finish
before the script ends.

`patch`, `write` and `edit` are exclusive: each waits for every call started before it and holds
back every call after it. A `find` before a patch has finished when it writes, and a `run` after it
sees the patched files.

Identical read-only calls (`find`, `show`, `outline`, `files`, `follow`, `diff`, `log`, `blame`,
`read`, `stat`, `exists`) in one run return the first result, with the same arguments. Any write,
`run()` or MCP call clears that memo. `run`, `fetch`, `sleep`, `env` and MCP calls are never
memoized: their results can differ between identical calls.

The 120 s deadline drops calls that have not started and kills each `run()` command at that time
(its `timeout` is cut to the time left), so the script exits with the timeout error within about 2 s.

## Output

The output is capped at 400 lines and 24000 bytes. Change the line limit with `--max-output N` on
`acus script`, or the config:

```toml
[script.output]
lines = 400
bytes = 24000
```

Prints come first; the return value keeps half the budget. A cut ends with a footer that names
`.head()`, `.grep()` and `--max-output`. The byte limit scales with the line limit.

## Permissions

A script obeys the deny and ask rules of Claude Code's settings: managed, user, project and local.
A call that a rule denies or asks about is refused with a hint, because a script cannot prompt. An
ask gives `a script cannot prompt; run it as its own tool call so Claude Code asks, or remove the
rule`. Deny and ask apply in every mode, `bypassPermissions` included. A call no rule names goes
ahead, except as the permission mode says below.

The mode comes from the hooks `acus self setup` installs (`UserPromptSubmit` on each prompt,
`PostToolUse` when plan mode ends (`ExitPlanMode`), and `PreToolUse` when it carries one), which record it per session under the state directory in
`permission-mode/<session id>.json`. A mode not recorded, such as before the first prompt or without
the hooks, is not known, and a script then behaves as if there were no modes.

| Mode                                   | `run()`, `write`/`edit`/`patch`                                                                                          | `fetch`, MCP                 | `read`, `find`, `show` and the like |
| -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ | ---------------------------- | ----------------------------------- |
| `plan`                                 | refused: `refused in plan mode`                                                                                          | goes                         | goes                                |
| `dontAsk`                              | goes only with an allow rule                                                                                             | goes only with an allow rule | goes                                |
| `default`, `auto`, `acceptEdits`       | goes; with `[script] strict = true` as `dontAsk`, except that `acceptEdits` lets an edit inside the working directory go | same                         | goes                                |
| `bypassPermissions`, or mode not known | goes (`strict` has no effect)                                                                                            | goes                         | goes                                |

An edit of `.git/` or of a settings file Claude Code reads (`.claude/settings.json`,
`.claude/settings.local.json`, the user and managed files) needs an allow rule in every mode but
`bypassPermissions` and a mode not known, `strict` or not; the refusal says `protected path; run the
edit as its own tool call`.

A compound `run()` command needs an allow rule for every subcommand. `strict` with a mode not known
counts as `default`. A refusal for a missing allow rule says `no allow rule; add one or run it as its
own tool call`. Commands that `acceptEdits` approves in Claude Code (`mkdir`, `touch`, `rm`, `mv`,
`cp`, `sed`) still need an allow rule under `strict`. In `plan` every `run()` is refused, also a
read-only one: run it as its own tool call.

| Rule                                                 | Applies to                                                                                                                                                                                                                        |
| ---------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Bash(...)`                                          | `run()`, and `run` lines of a batch; sees through shell keywords, `{ }`, `!`, `( )` and `bash -c`/`sh -c`/`eval` bodies (also behind `-o X`, `-ec`, `--`), not `xargs`/`exec`/`find -exec`; the acus guard still applies after it |
| `Read(...)`                                          | `read`, `stat`, `exists`, and the explicit file path of `show`, `outline`, `blame`, `follow` or `find`; paths outside the repository and `permissions.additionalDirectories` are refused                                          |
| `Edit(...)`, `Write(...)`                            | `write`, `edit`, `patch`                                                                                                                                                                                                          |
| `WebFetch(domain:...)`                               | `fetch`, checked at every redirect hop                                                                                                                                                                                            |
| `mcp__SERVER`, `mcp__SERVER__*`, `mcp__SERVER__TOOL` | MCP tools                                                                                                                                                                                                                         |

A directory-wide `find`, `files` or `diff` does not filter denied files. String-form `patch()`
refuses paths outside the repository.

`fetch` is on by default and reads an http or https page as markdown. `[script] fetch_deny` lists
hosts it refuses: `host` exactly, or `*.host` for its subdomains. Turn it off with
`[commands] disabled = ["fetch"]` or `ACUS_DISABLE=fetch`.

No script function searches the web. Claude's WebSearch has no equivalent here; import an MCP server
that searches under `[script] mcp`, or call a search CLI with `run()`.

## Saved scripts

A script you will run again is an ES module:

```bash
acus script save fix-imports --note 'first version' <<'JS'
export const description = 'Lists files that import a module';
export const params = {module: 'module path to look for'};
export default async function (args) {
  return await find(`from '${args.module}'`, {filesWithMatches: true});
}
JS
acus script fix-imports module=./util
```

| Task                                       | Command                                                                 |
| ------------------------------------------ | ----------------------------------------------------------------------- |
| Save a version (`--global` for every repo) | `acus script save NAME --note TEXT`                                     |
| Run the latest, or version 2               | `acus script NAME key=value…`, `acus script NAME@2 …`                   |
| List, read, see the history                | `acus script list`, `acus script show NAME[@V]`, `acus script log NAME` |

- `args` holds the `key=value` words as strings; a repeated key makes an array. Write a script with
  `acus script --check` first if it is long; `save` also checks it.
- `save` refuses a module without `description`, `params` and a default function. Names use
  `a-z0-9-`; `save`, `list`, `show` and `log` are taken.
- Versions are immutable. Saving identical content adds none; `--note` is required and becomes the
  changelog line. Repo scripts live in `.acus/scripts/`, which ignores itself in git; global ones
  beside the config file. A repo script overrides a global one of the same name.
- Import another saved script with `import f from "repo:name@2"` or `"user:name"`. Relative and
  bare imports are refused.
- A session in a repo with saved scripts starts with a line `scripts here: a, b`; a script unused
  for 30 days drops out of it.

## MCP servers

A script can import the MCP servers defined in Claude Code's settings, once you opt in:

```toml
# acus config.toml
[script]
mcp = ["github", "context7"]
```

```js
import gh from "mcp:github";
import { tools } from "mcp:github"; // [{name, description}], to discover what exists
const issue = await gh.create_issue({ title: "x", body: "y" });
return issue;
```

- Each tool is an async method of the default export and returns `structuredContent` if the server
  sent it, else the text joined, else the raw content array. A tool error throws with its text.
- Leading `import` lines work in inline scripts too; each becomes an `await import()` on its line.
- Servers come from `projects[<dir>].mcpServers` in `~/.claude.json` (or `$CLAUDE_CONFIG_DIR`),
  then the repo's `.mcp.json`, then the user's `mcpServers`; the first scope that defines the name
  wins. `${VAR}` and `${VAR:-default}` expand as in Claude Code.
- A server not listed under `[script] mcp` throws with the config key in the hint, and nothing
  starts. Deny and ask rules for the server or tool refuse the call, as in Permissions.
- Only stdio servers work. http and sse servers throw "transport not supported yet", and OAuth is
  not available; reach those through their own CLI with `run()`.
- A server starts at its first import, serves one request at a time (servers run in parallel), and
  is killed with its process group when the script ends or its deadline passes.

## Batch

`acus batch` runs several acus commands at once without JavaScript. It reads stdin, one command per
line, without the `acus` prefix:

```bash
acus batch <<'EOF'
find 'fn parse' --block
show src/parser.rs:40-80
run 'cargo test' --tail 30
EOF
```

- Each line runs as its own process, in parallel. Output prints under a `== command` header per
  line, in line order. Blank lines and `#` comments are skipped; quotes work as in a shell.
- `--head N` caps each line's output at N lines (default 100 for commands that take `--head`), and `--cut N` clips each printed line to N characters. A line that sets its own `--head` or `--full` keeps it.
- The output is cut at about 24000 characters in all. A `failed:` footer repeats each failing line's
  reason.
- Refused, and the other lines still run: `patch` (use `acus patch` on its own), `batch` and
  `script` (no nesting), and `self` commands. A `run` line obeys the Bash rules and the guard, as
  `run()` does.
- Exit: 0 when a line found something, 1 when none did, 2 when a line failed.
- `acus run <<'EOF'` is not a batch. `run` takes one command, never reads stdin, and runs the
  project's test command.

## Common mistakes

- A patch with backticks or `${` in a JS string breaks the string: apply it with `acus patch`.
- Printing every intermediate result brings back the context cost the script saves: return or print
  the filtered result.
- A function without `await` returns a promise, not the result: write `await find(...)`.
- `parse.csv` and `readJson` return values: do not `JSON.parse(await read(...))`; the hint is in
  their errors, and `readJson` is refused by the same Read rules as `read`.
- Naming `return` or `print` anywhere turns off the implicit result: the last expression then prints
  nothing.
- Limits: 120 s of script time (`[script] timeout` in the acus config), 256 MB of memory, 1 MB of
  stack. There is no filesystem, network or process access besides these functions and the MCP
  servers; `patch`, `write` and `edit` are the only way to write.
