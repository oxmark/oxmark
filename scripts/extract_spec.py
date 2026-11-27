#!/usr/bin/env python3
"""Extract the examples of a CommonMark-style spec.txt into JSON.

Usage: python3 scripts/extract_spec.py spec.txt > tests/fixtures/commonmark.json
"""
import json
import re
import sys

FENCE = "`" * 32


def extract(text):
    examples = []
    section = ""
    lines = text.splitlines(keepends=True)
    i = 0
    number = 0
    while i < len(lines):
        line = lines[i]
        if line.startswith(FENCE + " example"):
            extension = line[len(FENCE + " example"):].strip()
            i += 1
            md = []
            while lines[i] != ".\n":
                md.append(lines[i])
                i += 1
            i += 1
            html = []
            while not lines[i].startswith(FENCE):
                html.append(lines[i])
                i += 1
            number += 1
            examples.append({
                "example": number,
                "section": section,
                "extension": extension,
                "markdown": "".join(md).replace("→", "\t"),
                "html": "".join(html).replace("→", "\t"),
            })
        else:
            m = re.match(r"^#{1,6} (.*)$", line)
            if m:
                section = m.group(1).strip()
        i += 1
    return examples


if __name__ == "__main__":
    with open(sys.argv[1], encoding="utf-8") as f:
        print(json.dumps(extract(f.read()), indent=1, ensure_ascii=False))
