#!/usr/bin/env bash
set -Eeuo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

PAYLOAD=".agent/wave.patch.gz.b64"
CHECKSUM=".agent/wave.sha256"
MESSAGE_FILE=".agent/message.txt"

for path in "$PAYLOAD" "$CHECKSUM" "$MESSAGE_FILE"; do
    if [[ ! -f "$path" ]]; then
        printf 'Missing required agent-wave file: %s\n' "$path" >&2
        exit 1
    fi
done

TMPDIR_LOCAL="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_LOCAL"' EXIT

PATCH_GZ="$TMPDIR_LOCAL/wave.patch.gz"
PATCH="$TMPDIR_LOCAL/wave.patch"

python3 - "$PAYLOAD" "$CHECKSUM" "$PATCH_GZ" <<'PY'
from __future__ import annotations

import base64
import hashlib
import sys
from pathlib import Path

payload_path = Path(sys.argv[1])
checksum_path = Path(sys.argv[2])
output_path = Path(sys.argv[3])

encoded = "".join(payload_path.read_text(encoding="utf-8").split())
try:
    decoded = base64.b64decode(encoded, validate=True)
except Exception as exc:
    raise SystemExit(f"Invalid base64 payload: {exc}") from exc

expected = checksum_path.read_text(encoding="utf-8").strip().split()[0].lower()
actual = hashlib.sha256(decoded).hexdigest()

if actual != expected:
    raise SystemExit(
        f"SHA-256 mismatch:\n"
        f"  expected: {expected}\n"
        f"  actual:   {actual}"
    )

output_path.write_bytes(decoded)
print(f"Verified agent-wave payload: {actual}")
PY

gzip --decompress --stdout "$PATCH_GZ" > "$PATCH"

if git apply --check --whitespace=error-all "$PATCH"; then
    printf 'Applying stranded agent-wave patch...\n'
    git apply --index --whitespace=error-all "$PATCH"
elif git apply --reverse --check "$PATCH"; then
    printf 'Patch contents already appear to be applied; continuing with cleanup.\n'
else
    printf 'The patch is neither cleanly applicable nor already applied.\n' >&2
    printf 'Inspect the decoded patch at: %s\n' "$PATCH" >&2
    exit 1
fi

COMMIT_MESSAGE="$(cat "$MESSAGE_FILE")"
if [[ -z "${COMMIT_MESSAGE//[[:space:]]/}" ]]; then
    COMMIT_MESSAGE="Recover stranded agent wave locally"
fi

rm -rf .agent
rm -f .github/workflows/apply-agent-wave.yml

mkdir -p ci/github-actions-disabled
shopt -s nullglob
for workflow in .github/workflows/*; do
    name="$(basename "$workflow")"
    git mv "$workflow" "ci/github-actions-disabled/${name}.disabled"
done
shopt -u nullglob

rmdir .github/workflows 2>/dev/null || true
rmdir .github 2>/dev/null || true

git add -A
git diff --cached --check
git commit -m "$COMMIT_MESSAGE"

printf '\nRecovered agent wave in commit:\n'
git --no-pager show --stat --oneline HEAD
