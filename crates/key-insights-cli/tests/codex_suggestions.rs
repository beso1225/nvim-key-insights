use std::io::Cursor;

use serde_json::Value;

use key_insights::{
    CodexSuggestionError, SuggestionAction, analyze_jsonl, analyze_jsonl_with_snapshot,
    parse_keymap_snapshot, render_codex_suggestions_markdown, validate_codex_suggestions_json,
    validate_codex_suggestions_json_for_summary,
};

const GLOBAL_GG: &str =
    "mapping-v1:a27261baf28b456378725590385ed469ee8c2c2e3fd5173cd32c7dbec271cc71";
const GLOBAL_G: &str =
    "mapping-v1:494845698ff45708f6996ca041b292cbe37a38c30e46af662058ec44d0ba2e67";

const VALID: &str = r#"{
  "schema_version": 1,
  "suggestions": [{
    "action": "learn_existing",
    "title": "Use the existing motion",
    "rationale": "The motion is already available and repeatedly observed.",
    "evidence": [{"metric": "repeated_key_runs", "value": 8}],
    "collision_check": {"checked": true, "conflicting_mapping_ids": []}
  }]
}"#;

fn summary() -> key_insights::AnalysisSummary {
    analyze_jsonl(Cursor::new(concat!(
        "{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"s\",\"elapsed_ms\":0}\n",
        "{\"schema_version\":1,\"event_type\":\"key_sequence\",\"session_id\":\"s\",\"elapsed_ms\":1,\"mode\":\"normal\",\"keys\":[\"j\",\"j\",\"x\"],\"duration_ms\":1}\n",
        "{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"s\",\"elapsed_ms\":2}\n",
    )))
    .expect("summary")
}

#[test]
fn output_schema_mirrors_the_rust_measurement_and_mapping_contract() {
    let schema: Value =
        serde_json::from_str(include_str!("../../../codex/suggestions.schema.json"))
            .expect("suggestion schema is valid JSON");
    let suggestion = &schema["properties"]["suggestions"]["items"];
    let metrics = suggestion["properties"]["evidence"]["items"]["properties"]["metric"]["enum"]
        .as_array()
        .expect("metric enum");
    let expected_metrics = [
        "sessions",
        "events",
        "total_session_duration_ms",
        "key_sequences",
        "sequence_keys",
        "text_runs",
        "text_keys",
        "mode_transitions",
        "mapping_uses",
        "repeated_key_runs",
        "repeated_key_presses",
        "unique_keys",
        "unique_mappings",
        "unique_repeated_keys",
        "observed_mappings",
        "unobserved_mappings",
        "total_snapshot_mappings",
        "count_prefix_occurrences",
        "count_prefix_digit_presses",
    ];
    assert_eq!(
        metrics,
        &expected_metrics
            .into_iter()
            .map(|metric| Value::String(metric.to_owned()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        suggestion["properties"]["collision_check"]["properties"]["conflicting_mapping_ids"]["items"]
            ["pattern"],
        "^mapping-v1:[0-9a-f]{64}$"
    );
    assert_eq!(
        suggestion["properties"]["mapping"]["properties"]["target_mapping_id"]["pattern"],
        "^mapping-v1:[0-9a-f]{64}$"
    );
    assert_eq!(suggestion["properties"]["title"]["maxLength"], 256);
    assert_eq!(suggestion["properties"]["rationale"]["maxLength"], 4096);
    assert_eq!(
        suggestion["properties"]["mapping"]["properties"]["lhs"]["items"]["maxLength"],
        64
    );
    assert_eq!(
        suggestion["properties"]["mapping"]["type"],
        serde_json::json!(["object", "null"])
    );
    assert_eq!(
        suggestion["properties"]["mapping"]["properties"]["target_mapping_id"]["type"],
        serde_json::json!(["string", "null"])
    );
    assert!(suggestion["allOf"].is_null());
}

#[test]
fn accepts_null_optional_fields_from_strict_codex_schema() {
    let strict = VALID.replace("\"evidence\"", "\"mapping\":null,\"evidence\"");
    let document = validate_codex_suggestions_json(strict.as_bytes()).expect("strict suggestions");
    assert_eq!(document.suggestions[0].mapping, None);
}

#[test]
fn validates_a_bounded_evidence_bound_suggestion_document() {
    let document = validate_codex_suggestions_json(VALID.as_bytes()).expect("valid suggestions");
    assert_eq!(document.suggestions.len(), 1);
    assert_eq!(
        document.suggestions[0].action,
        SuggestionAction::LearnExisting
    );
}

#[test]
fn rejects_unknown_fields_and_missing_evidence() {
    let unknown = VALID.replace(
        "\"schema_version\": 1",
        "\"schema_version\": 1, \"secret\": \"/Users/private\"",
    );
    assert!(matches!(
        validate_codex_suggestions_json(unknown.as_bytes()),
        Err(CodexSuggestionError::InvalidJson)
    ));
    let missing = VALID.replace(
        "\"evidence\": [{\"metric\": \"repeated_key_runs\", \"value\": 8}]",
        "\"evidence\": []",
    );
    assert!(matches!(
        validate_codex_suggestions_json(missing.as_bytes()),
        Err(CodexSuggestionError::InvalidContract { .. })
    ));
}

#[test]
fn rejects_collision_blind_mapping_actions() {
    let mapping = VALID
        .replace("learn_existing", "add_mapping")
        .replace(
            "\"evidence\"",
            "\"mapping\":{\"mode\":\"normal\",\"scope\":\"global\",\"lhs\":[\"g\",\"g\"]},\"evidence\"",
        )
        .replace("\"checked\": true", "\"checked\": false");
    assert!(matches!(
        validate_codex_suggestions_json(mapping.as_bytes()),
        Err(CodexSuggestionError::InvalidContract {
            field: "collision_check.checked"
        })
    ));
}

#[test]
fn rejects_unsupported_actions_and_non_measurement_evidence() {
    let action = VALID.replace("learn_existing", "invent_mapping");
    assert!(matches!(
        validate_codex_suggestions_json(action.as_bytes()),
        Err(CodexSuggestionError::InvalidJson)
    ));
    let metric = VALID.replace("repeated_key_runs", "path");
    assert!(matches!(
        validate_codex_suggestions_json(metric.as_bytes()),
        Err(CodexSuggestionError::InvalidContract {
            field: "evidence.metric"
        })
    ));
}

#[test]
fn rejects_histogram_names_as_scalar_evidence() {
    for metric in [
        "session_duration_ms",
        "sequence_length_keys",
        "average_inter_key_latency_ms",
    ] {
        let document = VALID.replace("repeated_key_runs", metric);
        assert!(matches!(
            validate_codex_suggestions_json(document.as_bytes()),
            Err(CodexSuggestionError::InvalidContract {
                field: "evidence.metric"
            })
        ));
    }
}

#[test]
fn renders_only_validated_suggestions_deterministically() {
    let summary = summary();
    let value = summary.repeated_key_runs;
    let json = VALID.replace("value\": 8", &format!("value\": {value}"));
    let document = validate_codex_suggestions_json_for_summary(json.as_bytes(), &summary, None)
        .expect("valid suggestions");
    assert_eq!(
        render_codex_suggestions_markdown(&document).expect("validated document"),
        format!(
            "# Codex suggestions\n\n## 1. Use the existing motion\n\n- **Action:** `learn_existing`\n- **Rationale:** The motion is already available and repeatedly observed.\n- **Evidence:**\n  - `repeated_key_runs`: {value}\n- **Collision check:** passed (no conflicting mappings reported)\n\n"
        )
    );
}

#[test]
fn binds_evidence_and_mapping_actions_to_the_summary_boundary() {
    let summary = summary();
    assert!(matches!(
        validate_codex_suggestions_json_for_summary(VALID.as_bytes(), &summary, None),
        Err(CodexSuggestionError::InvalidContract {
            field: "evidence.value"
        })
    ));
    let mapping = VALID
        .replace("learn_existing", "add_mapping")
        .replace(
            "\"evidence\"",
            "\"mapping\":{\"mode\":\"normal\",\"scope\":\"global\",\"lhs\":[\"g\",\"g\"]},\"evidence\"",
        )
        .replace(
            "value\": 8",
            &format!("value\": {}", summary.repeated_key_runs),
        );
    assert!(matches!(
        validate_codex_suggestions_json_for_summary(mapping.as_bytes(), &summary, None),
        Err(CodexSuggestionError::InvalidContract {
            field: "collision_check.snapshot"
        })
    ));
}

#[test]
fn binds_mapping_proposals_to_actual_snapshot_collisions() {
    let snapshot = parse_keymap_snapshot(Cursor::new(format!(
        r#"{{"snapshot_version":1,"mappings":[{{"mapping_id":"{GLOBAL_GG}","mode":"normal","scope":"global","lhs":["g","g"]}}]}}"#
    )))
    .expect("snapshot");
    let summary = analyze_jsonl_with_snapshot(
        Cursor::new(concat!(
            "{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"s\",\"elapsed_ms\":0}\n",
            "{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"s\",\"elapsed_ms\":1}\n",
        )),
        &snapshot,
    )
    .expect("summary");
    let value = summary.repeated_key_runs;
    let collision_blind = VALID
        .replace("learn_existing", "add_mapping")
        .replace(
            "\"evidence\"",
            "\"mapping\":{\"mode\":\"normal\",\"scope\":\"global\",\"lhs\":[\"g\",\"g\"]},\"evidence\"",
        )
        .replace("value\": 8", &format!("value\": {value}"));
    assert!(matches!(
        validate_codex_suggestions_json_for_summary(
            collision_blind.as_bytes(),
            &summary,
            Some(&snapshot)
        ),
        Err(CodexSuggestionError::InvalidContract {
            field: "collision_check.conflicting_mapping_ids"
        })
    ));

    let collision_checked = collision_blind.replace(
        "\"conflicting_mapping_ids\": []",
        &format!("\"conflicting_mapping_ids\": [\"{GLOBAL_GG}\"]"),
    );
    validate_codex_suggestions_json_for_summary(
        collision_checked.as_bytes(),
        &summary,
        Some(&snapshot),
    )
    .expect("actual collision is reported");

    let prefix_blind = collision_blind.replace("\"lhs\":[\"g\",\"g\"]", "\"lhs\":[\"g\"]");
    assert!(matches!(
        validate_codex_suggestions_json_for_summary(
            prefix_blind.as_bytes(),
            &summary,
            Some(&snapshot)
        ),
        Err(CodexSuggestionError::InvalidContract {
            field: "collision_check.conflicting_mapping_ids"
        })
    ));
    validate_codex_suggestions_json_for_summary(
        prefix_blind
            .replace(
                "\"conflicting_mapping_ids\": []",
                &format!("\"conflicting_mapping_ids\": [\"{GLOBAL_GG}\"]"),
            )
            .as_bytes(),
        &summary,
        Some(&snapshot),
    )
    .expect("a proposed prefix reports the existing longer mapping");

    let short_snapshot = parse_keymap_snapshot(Cursor::new(format!(
        r#"{{"snapshot_version":1,"mappings":[{{"mapping_id":"{GLOBAL_G}","mode":"normal","scope":"global","lhs":["g"]}}]}}"#
    )))
    .expect("short snapshot");
    let short_summary = analyze_jsonl_with_snapshot(
        Cursor::new(concat!(
            "{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"s\",\"elapsed_ms\":0}\n",
            "{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"s\",\"elapsed_ms\":1}\n",
        )),
        &short_snapshot,
    )
    .expect("short summary");
    let longer_proposal = collision_blind.replace(
        "\"conflicting_mapping_ids\": []",
        &format!("\"conflicting_mapping_ids\": [\"{GLOBAL_G}\"]"),
    );
    validate_codex_suggestions_json_for_summary(
        longer_proposal.as_bytes(),
        &short_summary,
        Some(&short_snapshot),
    )
    .expect("a longer proposal reports the existing shorter mapping");

    let change = collision_blind
        .replace("add_mapping", "change_mapping")
        .replace(
            "\"lhs\":[\"g\",\"g\"]",
            &format!("\"lhs\":[\"g\",\"g\"],\"target_mapping_id\":\"{GLOBAL_GG}\""),
        );
    let change =
        validate_codex_suggestions_json_for_summary(change.as_bytes(), &summary, Some(&snapshot))
            .expect("change target is excluded from its own collision set");
    let markdown = render_codex_suggestions_markdown(&change).expect("change renders");
    assert_eq!(markdown.matches(GLOBAL_GG).count(), 1);
}

#[test]
fn rejects_duplicate_keys_and_deep_json_before_deserialization() {
    let duplicate = VALID.replace(
        "\"schema_version\": 1",
        "\"schema_version\": 1, \"schema_version\": 1",
    );
    assert!(matches!(
        validate_codex_suggestions_json(duplicate.as_bytes()),
        Err(CodexSuggestionError::InvalidJson)
    ));
    let nested = format!("{}1{}", "[".repeat(256), "]".repeat(256));
    assert!(matches!(
        validate_codex_suggestions_json(nested.as_bytes()),
        Err(CodexSuggestionError::InvalidJson)
    ));
}

#[test]
fn rejects_sensitive_text_and_oversized_documents() {
    let secret = VALID.replace("Use the existing motion", "Review src/.env");
    assert!(matches!(
        validate_codex_suggestions_json(secret.as_bytes()),
        Err(CodexSuggestionError::InvalidContract {
            field: "suggestion.title"
        })
    ));
    let oversized = vec![b' '; key_insights::MAX_CODEX_PAYLOAD_BYTES + 1];
    assert!(matches!(
        validate_codex_suggestions_json(&oversized),
        Err(CodexSuggestionError::TooLarge { .. })
    ));
}

#[test]
fn accepts_standalone_search_key_references_but_rejects_paths() {
    let search_key = VALID.replace("Use the existing motion", "Use / to search");
    validate_codex_suggestions_json(search_key.as_bytes()).expect("standalone search key");

    let key_alternatives = VALID.replace("Use the existing motion", "Use j/k navigation");
    validate_codex_suggestions_json(key_alternatives.as_bytes())
        .expect("keyboard alternatives are safe suggestion text");

    let search_prefix = VALID.replace("Use the existing motion", "Use /… to search");
    validate_codex_suggestions_json(search_prefix.as_bytes())
        .expect("search notation punctuation is safe suggestion text");

    let path = VALID.replace("Use the existing motion", "Review src/config.lua");
    assert!(matches!(
        validate_codex_suggestions_json(path.as_bytes()),
        Err(CodexSuggestionError::InvalidContract {
            field: "suggestion.title"
        })
    ));

    let absolute_path = VALID.replace("Use the existing motion", "Review /Users/private");
    assert!(matches!(
        validate_codex_suggestions_json(absolute_path.as_bytes()),
        Err(CodexSuggestionError::InvalidContract {
            field: "suggestion.title"
        })
    ));
}

#[test]
fn escapes_html_in_validated_markdown() {
    let summary = summary();
    let json = VALID
        .replace("Use the existing motion", "Prefer <img alt=\\\"motion\\\">")
        .replace(
            "value\": 8",
            &format!("value\": {}", summary.repeated_key_runs),
        );
    let document = validate_codex_suggestions_json_for_summary(json.as_bytes(), &summary, None)
        .expect("valid suggestions");
    let markdown = render_codex_suggestions_markdown(&document).expect("validated document");
    assert!(markdown.contains("\\<img alt=\"motion\"\\>"));
}

#[test]
fn renders_expanded_markdown_within_its_separate_bound() {
    let summary = summary();
    let suggestions = (0..55)
        .map(|index| {
            serde_json::json!({
                "action": "no_change",
                "title": format!("Keep setup {index}"),
                "rationale": "*".repeat(4096),
                "evidence": [{"metric": "sessions", "value": summary.sessions}],
                "collision_check": {"checked": true, "conflicting_mapping_ids": []},
            })
        })
        .collect::<Vec<_>>();
    let json = serde_json::to_vec(&serde_json::json!({
        "schema_version": 1,
        "suggestions": suggestions,
    }))
    .expect("serialize suggestions");
    assert!(json.len() < key_insights::MAX_CODEX_PAYLOAD_BYTES);
    let validated =
        validate_codex_suggestions_json_for_summary(&json, &summary, None).expect("valid input");
    let markdown = render_codex_suggestions_markdown(&validated).expect("expanded output renders");
    assert!(markdown.len() > key_insights::MAX_CODEX_PAYLOAD_BYTES);
    assert!(markdown.len() <= key_insights::MAX_RENDERED_SUGGESTIONS_BYTES);
}
