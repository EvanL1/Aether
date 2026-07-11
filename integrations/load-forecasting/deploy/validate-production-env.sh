#!/usr/bin/env bash
set -euo pipefail

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

image=${AETHER_LOAD_FORECASTING_IMAGE:-}
token=${AETHER_LOAD_FORECASTING_BEARER_TOKEN:-}
concurrency=${AETHER_LOAD_FORECASTING_MAX_CONCURRENCY:-1}
bundles=${AETHER_LOAD_FORECASTING_ARTIFACT_BUNDLES:-}

[[ $image =~ ^[^[:space:]]+@sha256:[0-9a-f]{64}$ ]] || \
    fail "AETHER_LOAD_FORECASTING_IMAGE must be pinned as name@sha256:<64 lowercase hex>"
[[ ${#token} -ge 32 && ${#token} -le 8192 ]] || \
    fail "AETHER_LOAD_FORECASTING_BEARER_TOKEN must contain 32..8192 characters"
[[ $token =~ ^[A-Za-z0-9._~+/=-]+$ ]] || \
    fail "AETHER_LOAD_FORECASTING_BEARER_TOKEN contains unsupported characters"
[[ $concurrency =~ ^[0-9]+$ ]] || \
    fail "AETHER_LOAD_FORECASTING_MAX_CONCURRENCY must be an integer"
(( concurrency >= 1 && concurrency <= 256 )) || \
    fail "AETHER_LOAD_FORECASTING_MAX_CONCURRENCY must be inside [1, 256]"
[[ -n $bundles ]] || fail "AETHER_LOAD_FORECASTING_ARTIFACT_BUNDLES is required"

python3 - "$bundles" <<'PY' || exit 1
import json
import posixpath
import re
import sys


def fail(message: str) -> None:
    print(f"ERROR: {message}", file=sys.stderr)
    raise SystemExit(1)


try:
    declarations = json.loads(sys.argv[1])
except json.JSONDecodeError:
    fail("AETHER_LOAD_FORECASTING_ARTIFACT_BUNDLES must be valid JSON")
if not isinstance(declarations, list) or not 1 <= len(declarations) <= 128:
    fail("AETHER_LOAD_FORECASTING_ARTIFACT_BUNDLES must be a non-empty array of at most 128 bundles")
required = {"kind", "family", "version", "expected_digest", "files"}
seen = set()
for declaration in declarations:
    if not isinstance(declaration, dict) or set(declaration) != required:
        fail("each artifact bundle must contain exactly kind, family, version, expected_digest, and files")
    identity = tuple(declaration[name] for name in ("kind", "family", "version"))
    if not all(isinstance(value, str) and value.strip() for value in identity):
        fail("artifact kind, family, and version must be non-empty strings")
    if identity in seen:
        fail("artifact bundle identities must be unique")
    seen.add(identity)
    if not isinstance(declaration["expected_digest"], str) or not re.fullmatch(
        r"sha256:[0-9a-f]{64}", declaration["expected_digest"]
    ):
        fail("each artifact expected_digest must be a lowercase SHA-256 digest")
    files = declaration["files"]
    if not isinstance(files, dict) or not files:
        fail("each artifact bundle must contain at least one file")
    if any(
        not isinstance(name, str)
        or not name.strip()
        or not isinstance(path, str)
        or not posixpath.isabs(path)
        or "\x00" in path
        for name, path in files.items()
    ):
        fail("artifact file names must be non-empty and container paths must be absolute")
PY

echo "Load-Forecasting production environment is structurally valid"
