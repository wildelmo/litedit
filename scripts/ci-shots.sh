#!/usr/bin/env bash
# Decode the screenshots that Windows CI prints as base64 into ./ci-shots/.
# Usage: scripts/ci-shots.sh [RUN_ID]   (default: latest run on the current branch)
set -euo pipefail
repo=wildelmo/litedit
run=${1:-$(gh run list -R "$repo" --workflow windows.yml --limit 1 --json databaseId -q '.[0].databaseId')}
out=${OUT:-ci-shots}
mkdir -p "$out"
gh run view "$run" -R "$repo" --log | sed -E 's/^[^\t]*\t[^\t]*\t[0-9TZ:.-]+ //' | python3 -c '
import sys, base64, os
out = sys.argv[1]; name = None; buf = []
for line in sys.stdin:
    line = line.strip()
    if line.startswith("BEGIN_PNG "):
        name, buf = line.split(" ", 1)[1], []
    elif line.startswith("END_PNG") and name:
        open(os.path.join(out, name), "wb").write(base64.b64decode("".join(buf)))
        print(os.path.join(out, name)); name = None
    elif name:
        buf.append(line)
' "$out"
