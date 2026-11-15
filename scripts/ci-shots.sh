#!/usr/bin/env bash
# Decode the screenshots that Windows CI prints as base64 into ./ci-shots/.
# Usage: scripts/ci-shots.sh [RUN_ID]   (default: latest run on the current branch)
set -euo pipefail
repo=wildelmo/litedit
run=${1:-$(gh run list -R "$repo" --workflow windows.yml --limit 1 --json databaseId -q '.[0].databaseId')}
out=${OUT:-ci-shots}
mkdir -p "$out"
gh run view "$run" -R "$repo" --log | sed -E 's/^[^\t]*\t[^\t]*\t[0-9TZ:.-]+ //' | python3 -c '
import sys, base64, binascii, os, re
display_out = sys.argv[1]; out = os.path.realpath(display_out)
name = None; buf = []; encoded_size = 0
max_encoded_size = 16 * 1024 * 1024
safe_name = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,123}\.png", re.IGNORECASE).fullmatch
device_name = re.compile(r"(con|prn|aux|nul|com[0-9]|lpt[0-9])(\..*)?", re.IGNORECASE).fullmatch
for line in sys.stdin:
    line = line.strip()
    if line.startswith("BEGIN_PNG "):
        candidate = line.split(" ", 1)[1]
        if not safe_name(candidate) or device_name(candidate):
            raise SystemExit("unsafe PNG marker name")
        name, buf, encoded_size = candidate, [], 0
    elif line == "END_PNG" and name:
        destination = os.path.realpath(os.path.join(out, name))
        try:
            contained = os.path.commonpath((out, destination)) == out
        except ValueError:
            contained = False
        if not contained:
            raise SystemExit("PNG destination escapes output directory")
        try:
            data = base64.b64decode("".join(buf), validate=True)
        except (binascii.Error, ValueError):
            raise SystemExit("invalid PNG base64") from None
        with open(destination, "wb") as image:
            image.write(data)
        print(os.path.join(display_out, name)); name = None
    elif name:
        encoded_size += len(line)
        if encoded_size > max_encoded_size:
            raise SystemExit("PNG payload exceeds size limit")
        buf.append(line)
' "$out"
