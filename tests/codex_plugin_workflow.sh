#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
key_insights=${KEY_INSIGHTS_BIN:-"$repo_root/target/debug/key-insights"}
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/key-insights-plugin-workflow.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT

private_session_canary='session-private-canary-9a77'
private_project_canary='project-private-canary-4b21'
private_report_canary='report-private-canary-71e0'
input="$work_dir/session.jsonl"
summary="$work_dir/summary.json"
report="$work_dir/report.md"
payload="$work_dir/payload.json"
suggestions="$work_dir/suggestions.json"
rendered="$work_dir/suggestions.md"
rendered_again="$work_dir/suggestions-again.md"

cat >"$input" <<EOF
{"schema_version":1,"event_type":"session_start","session_id":"$private_session_canary","elapsed_ms":0,"project_id":"$private_project_canary"}
{"schema_version":1,"event_type":"key_sequence","session_id":"$private_session_canary","elapsed_ms":10,"mode":"normal","keys":["j","j"],"duration_ms":5}
{"schema_version":1,"event_type":"session_end","session_id":"$private_session_canary","elapsed_ms":20}
EOF
chmod 600 "$input"

"$key_insights" analyze "$input" --summary "$summary" --report "$report"
"$key_insights" preview "$summary" --output "$payload"

python3 - "$repo_root/codex/payload.schema.json" "$payload" <<'PY'
import json
import pathlib
import sys
import jsonschema

schema = json.loads(pathlib.Path(sys.argv[1]).read_text())
payload = json.loads(pathlib.Path(sys.argv[2]).read_text())
jsonschema.Draft202012Validator(schema).validate(payload)
assert payload["summary"]["sessions"] == 1
PY

for canary in "$private_session_canary" "$private_project_canary" "$private_report_canary"; do
  if grep -Fq "$canary" "$payload"; then
    echo "private canary crossed the Codex payload boundary: $canary" >&2
    exit 1
  fi
done

cat >"$suggestions" <<'EOF'
{"schema_version":1,"suggestions":[{"action":"no_change","title":"Keep the current workflow","rationale":"The aggregate sample is too small to justify a change.","evidence":[{"metric":"sessions","value":1}],"collision_check":{"checked":true,"conflicting_mapping_ids":[]}}]}
EOF
chmod 600 "$suggestions"

"$key_insights" suggestions "$summary" --input "$suggestions" --output "$rendered"
"$key_insights" suggestions "$summary" --input "$suggestions" --output "$rendered_again"
cmp "$rendered" "$rendered_again"
grep -Fq '# Codex suggestions' "$rendered"

printf '%s\n' "$private_report_canary" >>"$report"
for canary in "$private_session_canary" "$private_project_canary" "$private_report_canary"; do
  if grep -Fq "$canary" "$rendered"; then
    echo "private canary crossed the rendered suggestion boundary: $canary" >&2
    exit 1
  fi
done

cp "$rendered" "$work_dir/preserved.md"
cat >"$work_dir/tampered.json" <<'EOF'
{"schema_version":1,"suggestions":[{"action":"no_change","title":"Forged evidence","rationale":"This value is not bound to the summary.","evidence":[{"metric":"sessions","value":2}],"collision_check":{"checked":true,"conflicting_mapping_ids":[]}}]}
EOF
chmod 600 "$work_dir/tampered.json"
if "$key_insights" suggestions "$summary" --input "$work_dir/tampered.json" --output "$rendered"; then
  echo 'tampered evidence unexpectedly passed contextual validation' >&2
  exit 1
fi
cmp "$rendered" "$work_dir/preserved.md"

cat >"$work_dir/no-snapshot-mapping.json" <<'EOF'
{"schema_version":1,"suggestions":[{"action":"add_mapping","title":"Unsafe mapping proposal","rationale":"A mapping proposal requires a sanitized snapshot.","mapping":{"mode":"normal","scope":"global","lhs":["g","g"]},"evidence":[{"metric":"sessions","value":1}],"collision_check":{"checked":true,"conflicting_mapping_ids":[]}}]}
EOF
chmod 600 "$work_dir/no-snapshot-mapping.json"
if "$key_insights" suggestions "$summary" --input "$work_dir/no-snapshot-mapping.json" --output "$rendered"; then
  echo 'mapping proposal without a snapshot unexpectedly passed validation' >&2
  exit 1
fi
cmp "$rendered" "$work_dir/preserved.md"

echo 'Codex plugin workflow: ok'
