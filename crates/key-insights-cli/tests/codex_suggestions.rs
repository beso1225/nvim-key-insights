use std::io::Cursor;

use key_insights::{
    CodexSuggestionError, SuggestionAction, analyze_jsonl, render_codex_suggestions_markdown,
    validate_codex_suggestions_json, validate_codex_suggestions_json_for_summary,
};

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
    let mapping = VALID.replace("learn_existing", "add_mapping").replace(
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
