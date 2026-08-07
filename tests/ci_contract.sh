#!/usr/bin/env bash
set -euo pipefail

workflow="${CI_WORKFLOW:-.github/workflows/ci.yml}"
taskfile="Taskfile.pkl"
workflow_contents=$(<"$workflow")

grep -Fq "persist-credentials: false" <<<"$workflow_contents" || {
  echo "checkout credentials must not persist into untrusted project steps" >&2
  exit 1
}

grep -Fq "Neovim 0.10 compatibility" <<<"$workflow_contents"
grep -Fq "v0.10.4/nvim-linux-x86_64.tar.gz" <<<"$workflow_contents"
grep -Fq "95aaa8e89473f5421114f2787c13ae0ec6e11ebbd1a13a1bd6fcf63420f8073f" <<<"$workflow_contents"
grep -Fq "sha256sum --check" <<<"$workflow_contents"
grep -Fq -- "-l tests/lua/run.lua" <<<"$workflow_contents"

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
