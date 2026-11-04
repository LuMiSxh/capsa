#!/usr/bin/env python3
"""Runs the skill evals in <tool>/skill/evals against Claude Code headless.

Each case runs `claude -p` on Haiku at low effort in a fresh copy of a small
Rust fixture with a git repository, one extra branch and one modified file.
A should-trigger case passes when the first tool call is the skill's `Skill`
tool or a Bash command whose first word is the tool name. A near-miss passes
when neither happens. A task case passes when a Bash command matches one of
its expected commands: the same tool and subcommand, the expected flags with
their values, and the expected arguments.

Usage:
  scripts/skill-evals.py --dry-run
  scripts/skill-evals.py --out DIR --direct
  scripts/skill-evals.py --mode all --repeat 3 --out DIR --direct
  scripts/skill-evals.py --rescore DIR/results.json

`default` mode loads the user's settings, hooks and installed skills, so the
acus CORE hook and the guard are active. `no-hooks` mode loads only project
settings and links the repository skill into the fixture, which tests the
description alone. `--bare` is not a mode: Claude Code's `--bare` skips user
skills and refuses OAuth, so it cannot run these evals on a subscription.

Results go to <out>/results.json, raw streams to <out>/raw/, and a summary
table to stdout. The runner exits 1 when a case failed, 2 on a runner error.
"""

import argparse
import concurrent.futures
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TOOLS = ("acus", "census")
KIND_SLUG = {"should_trigger": "trigger", "near_miss": "near-miss", "task": "task"}
KIND_ORDER = {"should_trigger": 0, "near_miss": 1, "task": 2}
MODES = ("default", "no-hooks")
ALLOWED_TOOLS = "Skill,Read,Glob,Grep,Bash(acus *),Bash(census *)"
DENIED_TOOLS = "Write,Edit"
DIRECT_ENV = ("ANTHROPIC_BASE_URL", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN")


def load_cases(tool):
    evals = ROOT / tool / "skill" / "evals"
    triggers = json.loads((evals / "triggers.json").read_text())
    tasks = json.loads((evals / "tasks.json").read_text())
    cases = []
    should = triggers.get("should_trigger") or []
    near = triggers.get("should_not_trigger") or triggers.get("near_miss") or []
    for index, item in enumerate(should, 1):
        cases.append({"kind": "should_trigger", "index": index, "query": _text(item),
                      "why": _why(item), "expected": None})
    for index, item in enumerate(near, 1):
        cases.append({"kind": "near_miss", "index": index, "query": _text(item),
                      "why": _why(item), "expected": None})
    for index, item in enumerate(tasks.get("tasks") or [], 1):
        cases.append({"kind": "task", "index": index,
                      "query": item.get("prompt") or item.get("task") or "",
                      "why": item.get("why"), "expected": item.get("expected") or []})
    return cases


def _text(item):
    if isinstance(item, str):
        return item
    return item.get("query") or item.get("task") or item.get("prompt") or ""


def _why(item):
    return item.get("why") if isinstance(item, dict) else None


def fixture_files():
    return {
        ".gitignore": ".claude/\ntarget/\n",
        "Cargo.toml": '[package]\nname = "fixture"\nversion = "0.1.0"\nedition = "2024"\n',
        "README.md": "# Fixture\n\nA small Rust project for the skill evals.\n",
        "src/lib.rs": (
            "// SPDX-License-Identifier: MPL-2.0\n"
            "//! Fixture library.\n\n"
            "pub mod parser;\n"
            "pub mod search;\n\n"
            "pub const old_timeout: u64 = 30;\n\n"
            "pub fn start() -> u64 {\n"
            '    let config = parser::parse_config("default");\n'
            "    parser::Parser::new(config.timeout).parse();\n"
            "    old_timeout\n"
            "}\n"
        ),
        "src/parser.rs": _parser_rs(),
        "src/a.rs": _a_rs(),
        "src/search/mod.rs": (
            "// SPDX-License-Identifier: MPL-2.0\n"
            "//! Fixture search.\n\n"
            "pub mod query;\n"
        ),
        "src/search/query.rs": (
            "// SPDX-License-Identifier: MPL-2.0\n"
            "//! Fixture query.\n\n"
            "use crate::parser::Parser;\n\n"
            "pub fn run_query(timeout: u64) -> u64 {\n"
            "    Parser::new(timeout).parse()\n"
            "}\n"
        ),
        "crates/alpha/Cargo.toml": (
            '[package]\nname = "alpha"\nversion = "0.1.0"\nedition = "2024"\n'
        ),
        "crates/alpha/src/lib.rs": (
            "// SPDX-License-Identifier: MPL-2.0\n"
            "//! Fixture crate alpha.\n\n"
            "pub fn alpha_one() -> u64 {\n"
            "    1\n"
            "}\n"
        ),
        "crates/beta/Cargo.toml": '[package]\nname = "beta"\nversion = "0.1.0"\nedition = "2024"\n',
        "crates/beta/src/lib.rs": (
            "// SPDX-License-Identifier: MPL-2.0\n"
            "//! Fixture crate beta.\n\n"
            "pub fn beta_one() -> u64 {\n"
            "    1\n"
            "}\n\n"
            "pub fn beta_two() -> u64 {\n"
            "    2\n"
            "}\n"
        ),
    }


def _parser_rs():
    lines = [
        "// SPDX-License-Identifier: MPL-2.0",
        "//! Fixture parser.",
        "",
        "pub struct Config {",
        "    pub timeout: u64,",
        "}",
        "",
        "pub fn parse_config(input: &str) -> Config {",
        "    let timeout = input.parse().unwrap_or(30);",
        "    Config { timeout }",
        "}",
        "",
        "pub struct Parser {",
        "    timeout: u64,",
        "}",
        "",
        "impl Parser {",
        "    pub fn new(timeout: u64) -> Self {",
        "        Self { timeout }",
        "    }",
        "",
        "    pub fn parse(&self) -> u64 {",
        "        self.timeout + 1",
        "    }",
        "",
        "    pub fn reset(&self) -> u64 {",
        "        0",
        "    }",
        "}",
    ]
    while len(lines) < 86:
        n = len(lines) + 1
        lines.append(f"pub fn helper_{n:02}(value: u64) -> u64 {{ value + {n} }}")
    return "\n".join(lines) + "\n"


def _a_rs():
    lines = [
        "// SPDX-License-Identifier: MPL-2.0",
        "//! Fixture source with a deletable block.",
        "",
    ]
    while len(lines) < 39:
        n = len(lines)
        lines.append(f"pub fn filler_{n:03}() -> u32 {{ {n} }}")
    lines += [
        "pub fn delete_me() {",
        "    let value = 40;",
        "    let next = value + 1;",
        "    next",
        "}",
    ]
    while len(lines) < 100:
        n = len(lines)
        lines.append(f"pub fn tail_{n:03}() -> u32 {{ {n} }}")
    return "\n".join(lines) + "\n"


def run_git(cwd, *args):
    proc = subprocess.run(["git", "-C", str(cwd), *args], capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} failed: {proc.stderr.strip()}")


def build_template(work):
    template = work / "fixture-template"
    if template.exists():
        shutil.rmtree(template)
    for rel, text in fixture_files().items():
        path = template / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    run_git(template, "init", "-q", "-b", "main")
    run_git(template, "config", "user.name", "Skill Evals")
    run_git(template, "config", "user.email", "skill-evals@example.invalid")
    run_git(template, "add", "-A")
    run_git(template, "commit", "-q", "-m", "fixture: initial")
    run_git(template, "checkout", "-q", "-b", "eval-work")
    parser = template / "src/parser.rs"
    parser.write_text(
        parser.read_text()
        + "\npub fn parse_config_with(input: &str) -> Config {\n    parse_config(input)\n}\n"
    )
    run_git(template, "add", "-A")
    run_git(template, "commit", "-q", "-m", "parser: add parse_config_with")
    query = template / "src/search/query.rs"
    query.write_text(query.read_text() + "\npub fn pending_query() -> u64 {\n    0\n}\n")
    return template


def command_startswith(command, tool):
    return re.match(rf"^{re.escape(tool)}(?:\s|$)", command.lstrip()) is not None


def skill_input(call):
    data = call.get("input") or {}
    value = data.get("skill") or data.get("name") or data.get("command") or ""
    return str(value)


def trigger_hit(call, tool):
    if call is None:
        return False
    if call.get("name") == "Skill" and skill_input(call).split(":")[0] == tool:
        return True
    if call.get("name") == "Bash":
        return command_startswith(str((call.get("input") or {}).get("command") or ""), tool)
    return False


def describe_call(call):
    if call is None:
        return None
    name = call.get("name") or "?"
    data = call.get("input") or {}
    if name == "Bash":
        detail = str(data.get("command") or "")
    elif name == "Skill":
        detail = skill_input(call)
    else:
        detail = json.dumps(data, sort_keys=True)
    detail = re.sub(r"\s+", " ", detail).strip()
    if len(detail) > 240:
        detail = detail[:237] + "..."
    return f"{name}({detail})"


def parse_stream(stdout):
    events = {"init": None, "result": None, "tool_calls": [], "hooks": [], "rate": None}
    for line in stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        kind = event.get("type")
        if kind == "system" and event.get("subtype") == "init":
            events["init"] = event
        elif kind == "system" and event.get("subtype") == "hook_response":
            events["hooks"].append(event)
        elif kind == "assistant":
            for block in event.get("message", {}).get("content", []):
                if block.get("type") == "tool_use":
                    events["tool_calls"].append(block)
        elif kind == "result":
            events["result"] = event
        elif kind == "rate_limit_event":
            events["rate"] = event.get("rate_limit_info")
    return events


def hook_rewrites(hooks):
    rewrites = []
    for hook in hooks:
        if not str(hook.get("hook_name", "")).startswith("PreToolUse"):
            continue
        output = hook.get("output") or hook.get("stdout") or ""
        try:
            data = json.loads(output)
        except (TypeError, json.JSONDecodeError):
            continue
        specific = data.get("hookSpecificOutput") or {}
        updated = specific.get("updatedInput") or data.get("updatedInput") or {}
        command = updated.get("command") if isinstance(updated, dict) else None
        if command:
            rewrites.append({"hook": hook.get("hook_name"), "command": str(command)})
    return rewrites


def value_present(flag, value, tokens):
    for index, token in enumerate(tokens):
        if token == f"{flag}={value}":
            return True
        if token == flag and index + 1 < len(tokens) and tokens[index + 1] == value:
            return True
    return False


def strip_separators(command):
    # Keep quoted text and heredocs intact so flags inside a pattern stay whole.
    if "<<" in command:
        return command
    out = []
    quote = None
    index = 0
    while index < len(command):
        char = command[index]
        if quote:
            out.append(char)
            if char == quote:
                quote = None
        elif char in "'\"":
            quote = char
            out.append(char)
        elif char in ";\n|":
            out.append(" ")
            if index + 1 < len(command) and command[index + 1] == char:
                index += 1
        elif char == "&" and index + 1 < len(command) and command[index + 1] == "&":
            out.append(" ")
            index += 1
        else:
            out.append(char)
        index += 1
    return "".join(out)


def command_matches_line(expected, command):
    try:
        want = shlex.split(expected)
        have = shlex.split(strip_separators(command))
    except ValueError:
        return expected in re.sub(r"\s+", " ", command)
    if len(want) < 2 or len(have) < 2:
        return expected in re.sub(r"\s+", " ", command)
    start = None
    for index in range(len(have) - 1):
        if Path(have[index]).name == want[0] and have[index + 1] == want[1]:
            start = index
            break
    if start is None:
        return False
    flat = re.sub(r"\s+", " ", strip_separators(command))
    index = 2
    while index < len(want):
        token = want[index]
        if token.startswith("-"):
            present = token in have or any(item.startswith(f"{token}=") for item in have)
            if not present:
                return False
            if index + 1 < len(want) and not want[index + 1].startswith("-"):
                if not value_present(token, want[index + 1], have):
                    return False
                index += 2
                continue
        elif token not in have and token not in flat:
            return False
        index += 1
    return True


def matches_expected(expected, command):
    lines = [line.strip() for line in expected.splitlines() if line.strip()]
    lines = [line for line in lines if not re.search(r"<[^<>]+>", line)]
    if not lines or not command_matches_line(lines[0], command):
        return False
    flat = re.sub(r"\s+", " ", command)
    position = 0
    for line in lines[1:]:
        needle = re.sub(r"\s+", " ", line).strip()
        found = flat.find(needle, position)
        if found < 0:
            return False
        position = found + len(needle)
    return True


def match_task(expected, commands):
    for want in expected:
        for command in commands:
            if matches_expected(want, command):
                return True, want
    return False, None


def build_command(args, prompt, max_turns, mode):
    command = [
        args.claude, "-p", prompt,
        "--model", args.model,
        "--effort", args.effort,
        "--output-format", "stream-json",
        "--verbose",
        "--no-session-persistence",
        "--max-turns", str(max_turns),
        "--permission-mode", "default",
        "--permission-prompts", "none",
        "--allowedTools", ALLOWED_TOOLS,
        "--disallowedTools", DENIED_TOOLS,
        "--strict-mcp-config",
    ]
    if mode == "no-hooks":
        command += ["--setting-sources", "project"]
    return command


def prepare_env(direct):
    env = dict(os.environ)
    if direct:
        for key in DIRECT_ENV:
            env.pop(key, None)
    return env


def make_case_dir(template, out, mode, job):
    case_dir = out / "work" / f"{mode}-{job['id']}-r{job['run']}"
    if case_dir.exists():
        shutil.rmtree(case_dir)
    shutil.copytree(template, case_dir)
    if mode == "no-hooks":
        skills = case_dir / ".claude" / "skills"
        skills.mkdir(parents=True, exist_ok=True)
        link = skills / job["skill"]
        link.symlink_to(ROOT / job["skill"] / "skill", target_is_directory=True)
    return case_dir


def run_job(job, template, args, env, out):
    case_dir = make_case_dir(template, out, job["mode"], job)
    max_turns = args.max_turns_trigger if job["kind"] != "task" else args.max_turns_task
    command = build_command(args, job["query"], max_turns, job["mode"])
    started = time.time()
    timed_out = False
    try:
        proc = subprocess.run(command, cwd=case_dir, env=env, capture_output=True,
                              text=True, encoding="utf-8", errors="replace",
                              timeout=args.timeout)
        stdout, stderr, code = proc.stdout, proc.stderr, proc.returncode
    except subprocess.TimeoutExpired as error:
        timed_out = True
        stdout = error.stdout or ""
        stderr = error.stderr or ""
        if isinstance(stdout, bytes):
            stdout = stdout.decode("utf-8", "replace")
        if isinstance(stderr, bytes):
            stderr = stderr.decode("utf-8", "replace")
        code = -1
    finally:
        if not args.keep:
            shutil.rmtree(case_dir, ignore_errors=True)

    raw = out / "raw" / f"{job['mode']}-{job['id']}-r{job['run']}.jsonl"
    raw.write_text(stdout)
    events = parse_stream(stdout)
    calls = events["tool_calls"]
    bash = [str((call.get("input") or {}).get("command") or "") for call in calls
            if call.get("name") == "Bash"]
    first = calls[0] if calls else None

    if job["kind"] == "should_trigger":
        passed = trigger_hit(first, job["skill"])
        matched = None
    elif job["kind"] == "near_miss":
        passed = not any(trigger_hit(call, job["skill"]) for call in calls)
        matched = None
    else:
        passed, matched = match_task(job["expected"], bash)

    result = events["result"] or {}
    init = events["init"] or {}
    return {
        "id": job["id"],
        "skill": job["skill"],
        "kind": job["kind"],
        "mode": job["mode"],
        "run": job["run"],
        "query": job["query"],
        "why": job["why"],
        "expected": job["expected"],
        "matched_expected": matched,
        "passed": bool(passed),
        "first_tool": describe_call(first),
        "tool_calls": [describe_call(call) for call in calls],
        "bash_commands": [command[:2000] for command in bash],
        "hook_rewrites": hook_rewrites(events["hooks"]),
        "model": init.get("model"),
        "skills_visible": job["skill"] in (init.get("skills") or []),
        "num_turns": result.get("num_turns"),
        "cost_usd": result.get("total_cost_usd"),
        "is_error": result.get("is_error"),
        "subtype": result.get("subtype"),
        "terminal_reason": result.get("terminal_reason"),
        "result_text": str(result.get("result") or "")[:600],
        "rate_limit": events["rate"],
        "duration_ms": int((time.time() - started) * 1000),
        "exit_code": code,
        "timed_out": timed_out,
        "stderr": stderr[-1000:],
        "raw": str(raw.relative_to(out)),
    }


def summarize(results):
    rows = {}
    for result in results:
        key = (result["mode"], result["skill"])
        row = rows.setdefault(key, {kind: [0, 0] for kind in KIND_ORDER})
        row[result["kind"]][1] += 1
        if result["passed"]:
            row[result["kind"]][0] += 1
    failures = [result for result in results if not result["passed"]]
    requests = sum(int(result["num_turns"] or 0) for result in results)
    cost = sum(float(result["cost_usd"] or 0) for result in results)
    rates = [result["rate_limit"] for result in results if result["rate_limit"]]
    return {"rows": rows, "failures": failures, "requests": requests,
            "cost_usd": round(cost, 6), "rate_limit": rates[-1] if rates else None}


def ratio(pair):
    passed, total = pair
    if total == 0:
        return "-"
    return f"{passed}/{total} ({round(100 * passed / total)}%)"


def print_summary(results, summary, modes):
    print(f"Skill evals: {', '.join(modes)} · Haiku / low · {len(results)} runs")
    print(f"{'mode':<9} {'skill':<7} {'trigger recall':<17} "
          f"{'near-miss spec':<17} {'task pass':<12}")
    for (mode, skill), row in sorted(summary["rows"].items()):
        print(f"{mode:<9} {skill:<7} {ratio(row['should_trigger']):<17} "
              f"{ratio(row['near_miss']):<17} {ratio(row['task']):<12}")
    total_passed = sum(1 for result in results if result["passed"])
    print(f"Totals: {total_passed}/{len(results)} runs passed; "
          f"{summary['requests']} requests; ${summary['cost_usd']:.4f}")
    rate = summary["rate_limit"] or {}
    windows = rate.get("unifiedWindows") or {}
    if windows:
        parts = [f"{name} {round(100 * data.get('utilization', 0))}%"
                 for name, data in windows.items()]
        print(f"Rate limits: {', '.join(parts)}")
    if summary["failures"]:
        print("Failures:")
        for result in summary["failures"]:
            first = result["first_tool"] or "no tool call"
            print(f"  [{result['mode']}] {result['id']}  first: {first}")


def dry_run(jobs, args):
    env_prefix = "env -u " + " -u ".join(DIRECT_ENV) if args.direct else ""
    for job in jobs:
        max_turns = args.max_turns_trigger if job["kind"] != "task" else args.max_turns_task
        command = build_command(args, job["query"], max_turns, job["mode"])
        prefix = f"{env_prefix} " if env_prefix else ""
        case_dir = args.out / "work" / f"{job['mode']}-{job['id']}-r{job['run']}"
        print(f"# {job['mode']} {job['id']} ({job['kind']})")
        print(f"  cd {shlex.quote(str(case_dir))}")
        print(f"  {prefix}{shlex.join(command)}")


def parse_args(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--dry-run", action="store_true", help="print the commands and exit")
    parser.add_argument("--repeat", type=int, default=1, help="runs per case (default 1)")
    parser.add_argument("--parallel", type=int, default=3, help="concurrent cases, at most 3")
    parser.add_argument("--out", type=Path,
                        default=Path(tempfile.gettempdir()) / "capsa-skill-evals",
                        help="results and work directory")
    parser.add_argument("--mode", choices=(*MODES, "all"), default="default",
                        help="settings/hooks profile (default: default)")
    parser.add_argument("--tool", choices=TOOLS, action="append", dest="tools",
                        help="limit to a tool (repeatable)")
    parser.add_argument("--only", action="append", default=[],
                        help="limit to cases whose id or query contains this text")
    parser.add_argument("--timeout", type=int, default=120, help="seconds per case")
    parser.add_argument("--claude", default=shutil.which("claude") or "claude",
                        help="claude binary")
    parser.add_argument("--model", default="haiku")
    parser.add_argument("--effort", default="low")
    parser.add_argument("--max-turns-trigger", type=int, default=1)
    parser.add_argument("--max-turns-task", type=int, default=2)
    parser.add_argument("--direct", action="store_true",
                        help="use the Anthropic OAuth login, dropping proxy and API-key env")
    parser.add_argument("--keep", action="store_true", help="keep the case directories")
    parser.add_argument("--rescore", type=Path,
                        help="re-score task cases in a results.json after a matcher change")
    args = parser.parse_args(argv)
    if args.repeat < 1:
        parser.error("--repeat must be at least 1")
    if not 1 <= args.parallel <= 3:
        parser.error("--parallel must be 1 to 3, to spare rate limits")
    if args.max_turns_trigger < 1 or args.max_turns_task < 1:
        parser.error("--max-turns must be at least 1")
    if args.rescore is None and shutil.which(args.claude) is None \
            and not Path(args.claude).exists():
        parser.error(f"claude binary not found: {args.claude}")
    return args


def build_jobs(args):
    modes = MODES if args.mode == "all" else (args.mode,)
    tools = tuple(args.tools) if args.tools else TOOLS
    jobs = []
    for mode in modes:
        for tool in tools:
            for case in load_cases(tool):
                case_id = f"{tool}-{KIND_SLUG[case['kind']]}-{case['index']:02d}"
                if args.only and not any(
                    needle.lower() in case_id.lower() or needle.lower() in case["query"].lower()
                    for needle in args.only
                ):
                    continue
                for run in range(1, args.repeat + 1):
                    jobs.append({"id": case_id, "skill": tool, "kind": case["kind"],
                                 "mode": mode, "run": run, "query": case["query"],
                                 "why": case["why"], "expected": case["expected"]})
    jobs.sort(key=lambda job: (MODES.index(job["mode"]), TOOLS.index(job["skill"]),
                               KIND_ORDER[job["kind"]], job["id"], job["run"]))
    return jobs


def rescore(path):
    if not path.exists():
        print(f"error: no results at {path}", file=sys.stderr)
        return 2
    payload = json.loads(path.read_text())
    runs = payload.get("runs") or []
    for run in runs:
        if run.get("kind") == "task":
            run["passed"], run["matched_expected"] = match_task(
                run.get("expected") or [], run.get("bash_commands") or []
            )
    summary = summarize(runs)
    payload["requests"] = summary["requests"]
    payload["cost_usd"] = summary["cost_usd"]
    payload["rate_limit"] = summary["rate_limit"]
    payload["summary"] = {
        f"{mode}/{skill}": {kind: ratio(pair) for kind, pair in row.items()}
        for (mode, skill), row in summary["rows"].items()
    }
    payload["failures"] = [run["id"] for run in summary["failures"]]
    path.write_text(json.dumps(payload, indent=2) + "\n")
    print_summary(runs, summary, payload.get("modes") or [])
    print(f"Results: {path}")
    return 1 if summary["failures"] else 0


def main(argv):
    args = parse_args(argv)
    args.out = args.out.resolve()
    if args.rescore is not None:
        return rescore(args.rescore.resolve())
    jobs = build_jobs(args)
    if not jobs:
        print("error: no cases matched", file=sys.stderr)
        return 2
    if args.dry_run:
        dry_run(jobs, args)
        print(f"# {len(jobs)} runs", file=sys.stderr)
        return 0
    args.out.mkdir(parents=True, exist_ok=True)
    work = args.out / "work"
    work.mkdir(exist_ok=True)
    (args.out / "raw").mkdir(exist_ok=True)
    try:
        template = build_template(work)
    except (OSError, RuntimeError) as error:
        print(f"error: could not build the fixture: {error}", file=sys.stderr)
        return 2
    env = prepare_env(args.direct)
    results = []
    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.parallel) as pool:
            futures = {pool.submit(run_job, job, template, args, env, args.out): job
                       for job in jobs}
            for future in concurrent.futures.as_completed(futures):
                job = futures[future]
                try:
                    results.append(future.result())
                except (OSError, RuntimeError, subprocess.SubprocessError) as error:
                    results.append({"id": job["id"], "skill": job["skill"],
                                    "kind": job["kind"], "mode": job["mode"],
                                    "run": job["run"], "query": job["query"],
                                    "why": job["why"], "expected": job["expected"],
                                    "matched_expected": None, "passed": False,
                                    "first_tool": None, "tool_calls": [],
                                    "bash_commands": [], "hook_rewrites": [],
                                    "model": None, "skills_visible": False,
                                    "num_turns": 0, "cost_usd": 0, "is_error": True,
                                    "result_text": "", "rate_limit": None,
                                    "duration_ms": 0, "exit_code": -1,
                                    "timed_out": False, "stderr": str(error), "raw": ""})
    finally:
        if not args.keep:
            shutil.rmtree(work, ignore_errors=True)
    results.sort(key=lambda result: (MODES.index(result["mode"]),
                                     TOOLS.index(result["skill"]),
                                     KIND_ORDER[result["kind"]], result["id"],
                                     result["run"]))
    summary = summarize(results)
    payload = {
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "model": args.model,
        "effort": args.effort,
        "repeat": args.repeat,
        "direct": args.direct,
        "modes": MODES if args.mode == "all" else (args.mode,),
        "tools": args.tools or list(TOOLS),
        "requests": summary["requests"],
        "cost_usd": summary["cost_usd"],
        "rate_limit": summary["rate_limit"],
        "summary": {f"{mode}/{skill}": {kind: ratio(pair) for kind, pair in row.items()}
                    for (mode, skill), row in summary["rows"].items()},
        "failures": [result["id"] for result in summary["failures"]],
        "runs": results,
    }
    results_path = args.out / "results.json"
    results_path.write_text(json.dumps(payload, indent=2) + "\n")
    print_summary(results, summary, payload["modes"])
    print(f"Results: {results_path}")
    return 1 if summary["failures"] else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
