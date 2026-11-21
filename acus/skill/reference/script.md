# script

Contents: [Functions](#functions), [Saved scripts](#saved-scripts), [Limits](#limits),
[MCP servers](#mcp-servers), [Common mistakes](#common-mistakes),
[Parallel calls](#parallel-calls).

```bash
acus script <<'JS'
const hits = await find('fn parse', {paths: ['src'], glob: '*.rs'});
print(hits.paths());
return await show(hits.head(2).map((h) => `${h.path}:${h.line}-${h.line + 20}`));
JS
acus script -e 'return await run("cargo test", {tail: 30})'
```

The program is the body of an async function: `await` and `return` work at the top level. `print`
values come first, then the returned value. Use a script when the next call is known before the
first one runs; when the next step depends on what you read, make single calls.

## Functions

Options are an object as the last argument; a key is the long flag name in camelCase
(`ignoreCase` is `--ignore-case`, a one-letter key is a short flag). `true` is a bare flag, an
array repeats it, `paths` adds paths after the positionals. The command's own parser checks the
flags, so `acus help <command>` lists them.

| Call | Returns |
|---|---|
| `find(pattern or [patterns], {paths, block, filesWithMatches, count, glob, ...})` | `hits` `{path, line, symbol?, text}`; `block` gives `views`, `filesWithMatches` and `count` give `files` |
| `show(...addresses)` | `views` `{path, start, end, symbol?, text}` |
| `outline(paths, {...})` | `outline` `{path, lang?, lines?, symbols}` |
| `files(paths, {glob, lines})` | `files` `{path, lines?}` |
| `follow(address, {down})`, `blame(address)` | `follow`, `blame` edges and runs |
| `diff(rev, {patch, paths})`, `log({n, ...})` | `diff` per file with symbols, `log` commits |
| `patch(text, {check, diff, run})` | `{kind: "patch", written, changes, notes, regions, run?}` |
| `run(cmd, {tail, grep, drop, timeout, json})` | `{kind: "run", ok, code, text, failures?, json?}` |
| `read(path)` | file text without line numbers, for data (JSON, TOML, lists) |

- List results are arrays carrying a `kind`. `.head(n)`, `.tail(n)`, `.grep(re)` keep it, and
  `.paths()` returns the distinct paths.
- Returning a `hits`, `views`, `files` or `run` value prints it like the command; `--json`
  prints `{prints, result}` with the objects.
- `run(cmd, {json: true})` parses the output as JSON; stderr is merged into it.
- `run` applies the guard's decision to the command: a refused command throws with the guard's
  hint, so use `read`, `show` or `find` for files. A command without `timeout` stops after 120 s.
- `read` refuses paths outside the repository.
- A failed call throws an `Error` with `.hint`; catch it, or let it end the script with
  `error: line N: ...`.

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

| Task | Command |
|---|---|
| Save a version (`--global` for every repo) | `acus script save NAME --note TEXT` |
| Run the latest, or version 2 | `acus script NAME key=value…`, `acus script NAME@2 …` |
| List, read, see the history | `acus script list`, `acus script show NAME[@V]`, `acus script log NAME` |

- `args` holds the `key=value` words as strings; a repeated key makes an array.
- `save` refuses a module without `description`, `params` and a default function. Names use
  `a-z0-9-`; `save`, `list`, `show` and `log` are taken.
- Versions are immutable. Saving identical content adds none; `--note` is required and becomes the
  changelog line. Repo scripts live in `.acus/scripts/`, which ignores itself in git; global ones
  beside the config file. A repo script overrides a global one of the same name.
- Import another saved script with `import f from "repo:name@2"` or `"user:name"`. Relative and
  bare imports are refused.
- A session in a repo with saved scripts starts with a line `scripts here: a, b`; a script unused
  for 30 days drops out of it.

## Limits

120 s of script time (`[script] timeout` in the acus config), 256 MB of memory, 1 MB of stack. There
is no filesystem, network or process access besides these functions and the MCP servers below;
`patch` is the only way to write.

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
  starts. `permissions.deny` and `permissions.ask` entries `mcp__SERVER`, `mcp__SERVER__*` and
  `mcp__SERVER__TOOL` in the user, project and local settings refuse the tool, because a script
  cannot prompt.
- Only stdio servers work. http and sse servers throw "transport not supported yet", and OAuth is
  not available; reach those through their own CLI with `run()`.
- A server starts at its first import, serves one request at a time (servers run in parallel), and
  is killed with its process group when the script ends or its deadline passes.

## Common mistakes

- A patch with backticks or `${` in a JS string breaks the string: apply it with `acus patch`.
- `run('cat f | head')` is refused as in Bash; call `show('f:1-20')`.
- Printing every intermediate result brings back the context cost the script saves: return the
  filtered result.
- A function without `await` returns a promise, not the result: write `await find(...)`.

## Parallel calls

Every function returns a promise and starts at once, so `await Promise.all([find(a), find(b),
run('cargo test')])` runs them together, up to 8 at a time. `patch` is the exception: it waits
for every call started before it and holds back every call after it, so a `find` before a patch
has finished when it writes, and a `run` after it sees the patched files. Calls you do not await
still finish before the script ends. At the 120 s deadline calls that have not started are dropped,
and each `run()` command is killed at that time (its `timeout` is cut to the time left), so the
script exits with the timeout error within about 2 s and leaves no process behind. A panic in a
host call is a script error in a debug build; a release build aborts the process (`panic =
"abort"`).
