#!/usr/bin/env bash
set -euo pipefail

workflow=".github/workflows/ci.yml"
taskfile="Taskfile.pkl"

grep -Fq "persist-credentials: false" "$workflow" || {
  echo "checkout credentials must not persist into untrusted project steps" >&2
  exit 1
}

if [[ $(grep -Fc -- "--no-update-lock-file" "$workflow") -lt 2 ]]; then
  echo "every Nix evaluation step must reject implicit flake.lock updates" >&2
  exit 1
fi

grep -Fq "local toolchainSources" "$taskfile" || {
  echo "pkfire tasks must declare shared toolchain inputs" >&2
  exit 1
}

grep -Fq '"flake.nix"' "$taskfile"
grep -Fq '"flake.lock"' "$taskfile"
grep -Fq "...toolchainSources" "$taskfile"

echo "CI security contract: ok"
