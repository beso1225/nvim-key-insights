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
snapshot="$work_dir/snapshot.json"
snapshot_summary="$work_dir/snapshot-summary.json"
snapshot_report="$work_dir/snapshot-report.md"
snapshot_payload="$work_dir/snapshot-payload.json"
snapshot_suggestions="$work_dir/snapshot-suggestions.json"
global_g='mapping-v1:494845698ff45708f6996ca041b292cbe37a38c30e46af662058ec44d0ba2e67'

cat >"$input" <<EOF
{"schema_version":1,"event_type":"session_start","session_id":"$private_session_canary","elapsed_ms":0,"project_id":"$private_project_canary"}
{"schema_version":1,"event_type":"key_sequence","session_id":"$private_session_canary","elapsed_ms":10,"mode":"normal","keys":["j","j"],"duration_ms":5}
{"schema_version":1,"event_type":"session_end","session_id":"$private_session_canary","elapsed_ms":20}
EOF
chmod 600 "$input"

"$key_insights" analyze "$input" --summary "$summary" --report "$report"
printf '%s\n' "$private_report_canary" >>"$report"
"$key_insights" preview "$summary" --output "$payload"

python3 - "$repo_root/codex/payload.schema.json" "$payload" <<'PY'
import copy
import json
import pathlib
import sys
import jsonschema

schema = json.loads(pathlib.Path(sys.argv[1]).read_text())
payload = json.loads(pathlib.Path(sys.argv[2]).read_text())
jsonschema.Draft202012Validator(schema).validate(payload)
assert payload["summary"]["sessions"] == 1
token_validator = jsonschema.Draft202012Validator(schema["$defs"]["token"])
assert token_validator.is_valid("<env>")
assert not token_validator.is_valid("<.env>")

mutations = []
path_token = copy.deepcopy(payload)
path_token["summary"]["keys"][0]["key"] = "/Users/alice/private/secret"
mutations.append(path_token)
threshold = copy.deepcopy(payload)
threshold["summary"]["ergonomics"]["thresholds"]["minimum_candidate_sessions"] = 0
mutations.append(threshold)
bucket = copy.deepcopy(payload)
bucket["summary"]["ergonomics"]["distributions"]["session_duration_ms"][0]["bucket"] = "forged"
mutations.append(bucket)
truncated_histogram = copy.deepcopy(payload)
truncated_histogram["summary"]["ergonomics"]["distributions"]["session_duration_ms"] = []
mutations.append(truncated_histogram)
forged_motion = copy.deepcopy(payload)
forged_motion["summary"]["ergonomics"]["repeated_motions"]["items"] = [
    {"motion": "x", "runs": 1, "presses": 3}
]
mutations.append(forged_motion)
missing_snapshot_version = copy.deepcopy(payload)
del missing_snapshot_version["summary"]["ergonomics"]["mapping_coverage"]["snapshot_version"]
mutations.append(missing_snapshot_version)
for mutation in mutations:
    assert not jsonschema.Draft202012Validator(schema).is_valid(mutation)
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

for invalid in unknown-field unknown-version malformed; do
  case "$invalid" in
    unknown-field)
      printf '%s\n' '{"schema_version":1,"suggestions":[],"unexpected":true}' >"$work_dir/$invalid.json"
      ;;
    unknown-version)
      printf '%s\n' '{"schema_version":2,"suggestions":[]}' >"$work_dir/$invalid.json"
      ;;
    malformed)
      printf '%s\n' '{"schema_version":1,"suggestions":[' >"$work_dir/$invalid.json"
      ;;
  esac
  chmod 600 "$work_dir/$invalid.json"
  if "$key_insights" suggestions "$summary" --input "$work_dir/$invalid.json" --output "$rendered"; then
    echo "$invalid suggestions unexpectedly passed validation" >&2
    exit 1
  fi
  cmp "$rendered" "$work_dir/preserved.md"
done

cat >"$snapshot" <<EOF
{"snapshot_version":1,"mappings":[{"mapping_id":"$global_g","mode":"normal","scope":"global","lhs":["g"]}]}
EOF
chmod 600 "$snapshot"
"$key_insights" analyze "$input" --summary "$snapshot_summary" --report "$snapshot_report" --keymap-snapshot "$snapshot"
"$key_insights" preview "$snapshot_summary" --output "$snapshot_payload"
python3 - "$repo_root/codex/payload.schema.json" "$snapshot_payload" <<'PY'
import json
import pathlib
import sys
import jsonschema

schema = json.loads(pathlib.Path(sys.argv[1]).read_text())
payload = json.loads(pathlib.Path(sys.argv[2]).read_text())
jsonschema.Draft202012Validator(schema).validate(payload)
assert len(payload["keymap_snapshot"]["mappings"]) == 1
invalid_attribution_id = json.loads(json.dumps(payload))
invalid_attribution_id["summary"]["mapping_attribution"]["mappings"][0]["mapping_id"] = "not-an-id"
assert not jsonschema.Draft202012Validator(schema).is_valid(invalid_attribution_id)
PY

cat >"$snapshot_suggestions" <<EOF
{"schema_version":1,"suggestions":[{"action":"add_mapping","title":"Consider a longer mapping","rationale":"The proposal accounts for the existing prefix before local validation.","mapping":{"mode":"normal","scope":"global","lhs":["g","g"]},"evidence":[{"metric":"sessions","value":1}],"collision_check":{"checked":true,"conflicting_mapping_ids":["$global_g"]}}]}
EOF
chmod 600 "$snapshot_suggestions"
"$key_insights" suggestions "$snapshot_summary" --input "$snapshot_suggestions" --output "$work_dir/snapshot-suggestions.md"
cp "$work_dir/snapshot-suggestions.md" "$work_dir/snapshot-preserved.md"

sed "s/\[\"$global_g\"\]/[]/" "$snapshot_suggestions" >"$work_dir/missing-collision.json"
chmod 600 "$work_dir/missing-collision.json"
if "$key_insights" suggestions "$snapshot_summary" --input "$work_dir/missing-collision.json" --output "$work_dir/snapshot-suggestions.md"; then
  echo 'missing collision unexpectedly passed validation' >&2
  exit 1
fi
cmp "$work_dir/snapshot-suggestions.md" "$work_dir/snapshot-preserved.md"

extra_id='mapping-v1:0000000000000000000000000000000000000000000000000000000000000000'
sed "s/\[\"$global_g\"\]/[\"$global_g\",\"$extra_id\"]/" "$snapshot_suggestions" >"$work_dir/extra-collision.json"
chmod 600 "$work_dir/extra-collision.json"
if "$key_insights" suggestions "$snapshot_summary" --input "$work_dir/extra-collision.json" --output "$work_dir/snapshot-suggestions.md"; then
  echo 'extra collision unexpectedly passed validation' >&2
  exit 1
fi
cmp "$work_dir/snapshot-suggestions.md" "$work_dir/snapshot-preserved.md"

echo 'Codex plugin workflow: ok'
