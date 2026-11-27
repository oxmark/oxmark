#!/usr/bin/env python3
"""Extracts Comark's SPEC examples into tests/fixtures/comark-spec.json.

Usage (needs PyYAML):
  python3 scripts/extract_comark_spec.py <comark>/packages/comark/SPEC \
      > tests/fixtures/comark-spec.json

Sections are read the way Comark's own runner (test/index.test.ts) reads them:
a `## Input`, `## AST` or `## HTML` header, a blank line, a fence, and a
closing fence that is the last one before the next section.
"""
import json
import os
import re
import sys

import yaml

HEADER = re.compile(r"^## (Input|AST|HTML|Markdown HTML|Markdown)\s*\n\n```(\w+)\n", re.M)
CLOSING = re.compile(r"```[^\S\n]*\n(?=\n## |[^\S\n]*$)")


def frontmatter(text):
    if not text.startswith("---\n"):
        return {}, text
    end = text.find("\n---\n", 4)
    if end == -1:
        return {}, text
    return yaml.safe_load(text[4:end]) or {}, text[end + 5:]


def sections(body):
    out = {}
    headers = list(HEADER.finditer(body))
    for i, m in enumerate(headers):
        start = m.end()
        end = headers[i + 1].start() if i + 1 < len(headers) else len(body)
        last = None
        for c in CLOSING.finditer(body, start):
            if c.start() >= end:
                break
            last = c
        content = body[start:last.start()] if last else body[start:end]
        out[m.group(1)] = content.strip()
    return out


def main(root):
    cases = []
    for dirpath, dirs, files in os.walk(root):
        dirs.sort()
        for name in sorted(files):
            if not name.endswith(".md") or name == "README.md":
                continue
            path = os.path.join(dirpath, name)
            with open(path, encoding="utf-8") as f:
                data, body = frontmatter(f.read())
            if data.get("skip"):
                continue
            s = sections(body)
            if "Input" not in s:
                continue
            case = {
                "name": os.path.relpath(path, root)[:-3],
                "options": data.get("options") or {},
                "markdown": s["Input"],
                "ast": json.loads(s["AST"]) if s.get("AST") else None,
                "html": s.get("HTML", ""),
            }
            cases.append(case)
    json.dump(cases, sys.stdout, ensure_ascii=False, indent=1)
    sys.stdout.write("\n")


main(sys.argv[1])
