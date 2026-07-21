#!/usr/bin/env bash
set -euo pipefail

workflow="${CI_WORKFLOW:-.github/workflows/ci.yml}"
taskfile="Taskfile.pkl"
workflow_contents=$(<"$workflow")

grep -Fq "persist-credentials: false" <<<"$workflow_contents" || {
  echo "checkout credentials must not persist into untrusted project steps" >&2
  exit 1
}

unprotected_nix_commands=$(awk '
  /nix[[:space:]]+(flake|develop)([[:space:]]|$)/ &&
    $0 !~ /--no-update-lock-file/ {
      print NR ":" $0
    }
' <<<"$workflow_contents")

if [[ -n "$unprotected_nix_commands" ]]; then
  echo "every Nix evaluation command must reject implicit flake.lock updates:" >&2
  echo "$unprotected_nix_commands" >&2
  exit 1
fi

grep -Fq "local toolchainSources" "$taskfile" || {
  echo "pkfire tasks must declare shared toolchain inputs" >&2
  exit 1
}

grep -Fq '"flake.nix"' "$taskfile"
grep -Fq '"flake.lock"' "$taskfile"
grep -Fq "...toolchainSources" "$taskfile"

grep -Fq 'pkfire.inputs.nixpkgs.follows = "nixpkgs"' flake.nix || {
  echo "pkfire must share the project nixpkgs closure" >&2
  exit 1
}

echo "CI security contract: ok"
