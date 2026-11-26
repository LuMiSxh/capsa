#!/usr/bin/env python3
"""Fails when a tracked file still holds a git conflict section.

A section is a labelled opener (`<<<<<<< HEAD`), a `=======` line, then a labelled closer
(`>>>>>>> branch`). The patch format's own `<<<<<<< SEARCH` and `>>>>>>> REPLACE` are ignored, and a
lone `=======` is not a section: a Markdown heading underlines with it.
"""

import subprocess
import sys


def main() -> int:
    files = subprocess.check_output(["git", "ls-files", "-z"]).split(b"\0")
    bad = []
    for raw in files:
        if not raw:
            continue
        path = raw.decode()
        try:
            text = open(path, encoding="utf-8").read()
        except (UnicodeDecodeError, OSError):
            continue
        opened = separator = False
        for number, line in enumerate(text.splitlines(), 1):
            line = line.rstrip()
            if line.startswith("<<<<<<< "):
                label = line[8:].strip()
                if label and label != "SEARCH":
                    opened, separator = True, False
            elif opened and line == "=======":
                separator = True
            elif opened and separator and line.startswith(">>>>>>> "):
                if line[8:].strip() != "REPLACE":
                    bad.append(f"{path}:{number}")
                    opened = separator = False
    if bad:
        print("\n".join(bad))
        print("resolve these conflict markers before committing", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
