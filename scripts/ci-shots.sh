#!/usr/bin/env bash
# Decode the screenshots that Windows CI prints as base64 into ./ci-shots/.
# Usage: scripts/ci-shots.sh [RUN_ID]
# Default: the latest run started by a push to the current branch. A run for a
# pull request is never picked by default, because its log is written by the
# pull request. Pass its RUN_ID to decode one, and do not trust the pictures.
set -euo pipefail
repo=wildelmo/litedit
run=${1:-}
if [ -z "$run" ]; then
  branch=$(git branch --show-current)
  if [ -z "$branch" ]; then
    echo "ci-shots: not on a branch; pass a RUN_ID" >&2
    exit 1
  fi
  run=$(gh run list -R "$repo" --workflow windows.yml --branch "$branch" --event push --limit 1 --json databaseId -q '.[0].databaseId')
  if [ -z "$run" ]; then
    echo "ci-shots: no run for a push to '$branch'; pass a RUN_ID" >&2
    exit 1
  fi
fi
case $run in
  *[!0-9]*)
    echo "ci-shots: RUN_ID must be a number" >&2
    exit 1
    ;;
esac
out=${OUT:-ci-shots}
mkdir -p "$out"
gh run view "$run" -R "$repo" --log | python3 -c '
import sys, base64, binascii, os, re
display_out = sys.argv[1]; out = os.path.realpath(display_out)
name = None; buf = []; encoded_size = 0; total_size = 0; pictures = 0
max_encoded_size = 16 * 1024 * 1024
max_total_size = 256 * 1024 * 1024
max_pictures = 500
png_signature = b"\x89PNG\r\n\x1a\n"
safe_name = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,123}\.png", re.IGNORECASE).fullmatch
device_name = re.compile(r"(con|prn|aux|nul|com[0-9]|lpt[0-9])(\..*)?", re.IGNORECASE).fullmatch
log_prefix = re.compile(r"^[^\t]*\t[^\t]*\t\ufeff?[0-9TZ:.-]+ ")
sys.stdin.reconfigure(encoding="utf-8", errors="replace")
for line in sys.stdin:
    line = log_prefix.sub("", line, count=1).strip()
    if line.startswith("BEGIN_PNG "):
        candidate = line.split(" ", 1)[1]
        if not safe_name(candidate) or device_name(candidate):
            raise SystemExit("unsafe PNG marker name")
        pictures += 1
        if pictures > max_pictures:
            raise SystemExit("too many PNG markers")
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
        if not data.startswith(png_signature):
            raise SystemExit("payload is not a PNG")
        with open(destination, "wb") as image:
            image.write(data)
        print(os.path.join(display_out, name)); name = None
    elif name:
        encoded_size += len(line)
        total_size += len(line)
        if encoded_size > max_encoded_size:
            raise SystemExit("PNG payload exceeds size limit")
        if total_size > max_total_size:
            raise SystemExit("PNG payloads exceed total size limit")
        buf.append(line)
' "$out"
