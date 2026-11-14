#!/usr/bin/env python3
"""Write THIRD-PARTY-NOTICES.txt: the licence of every crate compiled into the
Windows build, plus the bundled syntax definitions.

Usage: python3 scripts/third-party-notices.py   (from the repo root)
Re-run after changing dependencies.
"""
import json, os, subprocess, sys

TARGET = "x86_64-pc-windows-msvc"
meta = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--format-version", "1", "--filter-platform", TARGET]))
pkgs = {p["id"]: p for p in meta["packages"]}
nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}

# Crates linked into the exe: normal dependencies only (no build/dev tools).
seen, stack = set(), [meta["resolve"]["root"]]
while stack:
    i = stack.pop()
    if i in seen:
        continue
    seen.add(i)
    for d in nodes[i]["deps"]:
        if any(k["kind"] is None for k in d["dep_kinds"]):
            stack.append(d["pkg"])

def licence_texts(p):
    d = os.path.dirname(p["manifest_path"])
    out = []
    for f in sorted(os.listdir(d)):
        u = f.upper()
        if u.startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE")) and os.path.isfile(os.path.join(d, f)):
            with open(os.path.join(d, f), encoding="utf-8", errors="replace") as fh:
                out.append((f, fh.read().strip()))
    return out

crates = sorted((pkgs[i] for i in seen if pkgs[i]["name"] != "litedit"),
                key=lambda p: (p["name"], p["version"]))
lines = [
    "Third-party software in litedit",
    "===============================",
    "",
    "litedit is MIT-licensed (see LICENSE). It is designed after lite-xl (see",
    "NOTICE) and includes the following open-source components, each under its",
    "own licence, reproduced below.",
    "",
    "Summary",
    "-------",
]
for p in crates:
    lines.append(f"{p['name']} {p['version']}: {p.get('license') or 'see licence text'}")
# Identical texts (e.g. the Apache-2.0 licence) are printed once, with
# every crate that ships them listed above the text.
lines += ["", "Licence texts", "-------------", ""]
by_text = {}
missing = []
for p in crates:
    texts = licence_texts(p)
    if not texts:
        missing.append(p)
    for _, body in texts:
        key = " ".join(body.split())  # same text, different wrapping/spaces
        entry = by_text.setdefault(key, [body, []])
        entry[1].append(f"{p['name']} {p['version']}")
for body, users in by_text.values():
    lines.append("=" * 78)
    lines.append("Used by: " + ", ".join(dict.fromkeys(users)))
    lines.append("=" * 78)
    lines += ["", body, ""]
for p in missing:
    lines.append("=" * 78)
    lines.append(f"{p['name']} {p['version']}: licensed under {p.get('license')}"
                 f" ({p.get('repository') or ''}); the published crate ships no licence file.")
lines.append("")

ack = subprocess.check_output(["cargo", "run", "-q", "--example", "acknowledgements"], text=True)
lines += ["=" * 78, "Bundled syntax definitions (via two-face / bat)", "=" * 78, "", ack.strip(), ""]

with open("THIRD-PARTY-NOTICES.txt", "w", encoding="utf-8", newline="\n") as fh:
    fh.write("\n".join(lines))
print(f"{len(crates)} crates, {len(by_text)} distinct licence texts; no licence file in: "
      f"{', '.join(p['name'] for p in missing) or 'none'}", file=sys.stderr)
