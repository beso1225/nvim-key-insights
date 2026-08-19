use std::{fmt::Write, io::Cursor};

use key_insights::{
    CODEX_PAYLOAD_SCHEMA_VERSION, CodexPayloadError, MAX_CODEX_PAYLOAD_BYTES, analyze_jsonl,
    analyze_jsonl_with_snapshot, parse_keymap_snapshot, render_codex_payload_json,
};
use sha2::{Digest, Sha256};

const MAPPING_ID: &str =
    "mapping-v1:a27261baf28b456378725590385ed469ee8c2c2e3fd5173cd32c7dbec271cc71";

fn summary() -> key_insights::AnalysisSummary {
    analyze_jsonl(Cursor::new(concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"session-secret","elapsed_ms":0,"project_id":"/Users/private/project"}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"session-secret","elapsed_ms":1,"mode":"normal","keys":["j","j","x"],"duration_ms":1}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_end","session_id":"session-secret","elapsed_ms":2}"#,
        "\n",
    )))
    .expect("valid summary")
}

#[test]
fn renders_a_versioned_payload_from_sanitized_inputs_only() {
    let payload = render_codex_payload_json(&summary(), None).expect("payload renders");
    let value: serde_json::Value = serde_json::from_str(&payload).expect("strict JSON");

    assert_eq!(
        value["payload_schema_version"],
        CODEX_PAYLOAD_SCHEMA_VERSION
    );
    assert_eq!(value["purpose"], "analyze-neovim-usage");
    assert_eq!(value["instructions"]["evidence_required"], true);
    assert_eq!(value["instructions"]["collision_check_required"], true);
    assert!(value["summary"].is_object());
    assert!(value.get("keymap_snapshot").is_none());
    assert!(value.get("report").is_none());
    assert!(value.get("raw_log").is_none());
    assert!(!payload.contains("session-secret"));
    assert!(!payload.contains("/Users/private/project"));
    assert!(!payload.contains("project_id"));
    assert!(!payload.contains("mapping_rhs_secret"));
}

#[test]
fn canonical_payload_serialization_is_stable_and_compact() {
    let first = render_codex_payload_json(&summary(), None).expect("payload renders");
    let second = render_codex_payload_json(&summary(), None).expect("payload renders");

    assert_eq!(first, second);
    assert_eq!(
        format!("{:x}", Sha256::digest(first.as_bytes())),
        "9b91b3564b8d10092c3957665b2f71bbe31b86ca26c402e15d75946dd3622bc9"
    );
    assert!(first.starts_with(
        r#"{"payload_schema_version":1,"purpose":"analyze-neovim-usage","instructions":{"action_kinds":["learn_existing","add_mapping","change_mapping","no_change"],"evidence_required":true,"collision_check_required":true,"privacy_boundary":"#
    ));
    let instruction_position = first.find("\"instructions\"").expect("instructions field");
    let summary_position = first.find("\"summary\"").expect("summary field");
    assert!(instruction_position < summary_position);
    assert!(!first.contains('\n'));
    assert!(first.len() <= MAX_CODEX_PAYLOAD_BYTES);
}

#[test]
fn rejects_an_unsupported_summary_schema_before_serialization() {
    let mut summary = summary();
    summary.schema_version = 2;

    assert_eq!(
        render_codex_payload_json(&summary, None),
        Err(CodexPayloadError::UnsupportedSummarySchema { found: 2 })
    );
}

#[test]
fn rejects_an_unsupported_snapshot_schema_before_serialization() {
    let mut snapshot = parse_keymap_snapshot(Cursor::new(format!(
        r#"{{"snapshot_version":1,"mappings":[{{"mapping_id":"{MAPPING_ID}","mode":"normal","scope":"global","lhs":["g","g"]}}]}}"#
    )))
    .expect("valid snapshot");
    snapshot.snapshot_version = 2;

    assert_eq!(
        render_codex_payload_json(&summary(), Some(&snapshot)),
        Err(CodexPayloadError::UnsupportedSnapshotVersion { found: 2 })
    );
}

#[test]
fn rejects_mutated_sanitized_fields_and_nested_contract_versions() {
    let mut secret_summary = summary();
    secret_summary.keys[0].key = "/Users/private/secret".to_owned();
    assert!(matches!(
        render_codex_payload_json(&secret_summary, None),
        Err(CodexPayloadError::InvalidSummaryContract { .. })
    ));

    let mut versioned_summary = summary();
    versioned_summary.ergonomics.contract_version = 2;
    assert!(matches!(
        render_codex_payload_json(&versioned_summary, None),
        Err(CodexPayloadError::InvalidSummaryContract {
            field: "ergonomics.contract_version"
        })
    ));

    let mut snapshot = parse_keymap_snapshot(Cursor::new(format!(
        r#"{{"snapshot_version":1,"mappings":[{{"mapping_id":"{MAPPING_ID}","mode":"normal","scope":"global","lhs":["g","g"]}}]}}"#
    )))
    .expect("valid snapshot");
    snapshot.mappings[0].lhs = vec!["path/secret".to_owned()];
    assert_eq!(
        render_codex_payload_json(&summary(), Some(&snapshot)),
        Err(CodexPayloadError::InvalidSnapshot)
    );
}

#[test]
fn includes_only_the_sanitized_keymap_snapshot_fields() {
    let snapshot = parse_keymap_snapshot(Cursor::new(format!(
        r#"{{"snapshot_version":1,"mappings":[{{"mapping_id":"{MAPPING_ID}","mode":"normal","scope":"global","lhs":["g","g"]}}]}}"#
    )))
    .expect("valid snapshot");
    let summary = analyze_jsonl_with_snapshot(
        Cursor::new(concat!(
            r#"{"schema_version":1,"event_type":"session_start","session_id":"secret-session","elapsed_ms":0}"#,
            "\n",
            r#"{"schema_version":1,"event_type":"mapping_use","session_id":"secret-session","elapsed_ms":1,"mode":"normal","mapping_id":"mapping-v1:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff","typed_keys":["z"]}"#,
            "\n",
            r#"{"schema_version":1,"event_type":"session_end","session_id":"secret-session","elapsed_ms":2}"#,
            "\n",
        )),
        &snapshot,
    )
    .expect("valid summary");
    let payload = render_codex_payload_json(&summary, Some(&snapshot)).expect("payload renders");
    let value: serde_json::Value = serde_json::from_str(&payload).expect("strict JSON");

    assert_eq!(value["keymap_snapshot"]["snapshot_version"], 1);
    assert_eq!(
        value["keymap_snapshot"]["mappings"][0]["mapping_id"],
        MAPPING_ID
    );
    assert_eq!(
        value["keymap_snapshot"]["mappings"][0]["lhs"],
        serde_json::json!(["g", "g"])
    );
    assert!(value["keymap_snapshot"]["mappings"][0].get("rhs").is_none());
    assert!(
        value["keymap_snapshot"]["mappings"][0]
            .get("path")
            .is_none()
    );
    assert!(!payload.contains("secret-session"));
    assert_eq!(
        format!("{:x}", Sha256::digest(payload.as_bytes())),
        "882e81ff715512b5f48a11137484f59cea73a58ae6339d505f6528dcfcbb1678"
    );
}

#[test]
fn rejects_payloads_that_exceed_the_subprocess_size_limit() {
    let mut snapshot_json = String::from(r#"{"snapshot_version":1,"mappings":["#);
    for index in 0..2_000 {
        let token = format!("<C-{index:04}>");
        let mut preimage = String::new();
        write!(preimage, "10:mapping-v1").unwrap();
        write!(preimage, "6:normal").unwrap();
        write!(preimage, "6:global").unwrap();
        write!(preimage, "1:1").unwrap();
        write!(preimage, "{}:{token}", token.len()).unwrap();
        let mapping_id = format!("mapping-v1:{:x}", Sha256::digest(preimage.as_bytes()));
        if index != 0 {
            snapshot_json.push(',');
        }
        write!(
            snapshot_json,
            r#"{{"mapping_id":"{mapping_id}","mode":"normal","scope":"global","lhs":["{token}"]}}"#
        )
        .unwrap();
    }
    snapshot_json.push_str("]}");
    let snapshot = parse_keymap_snapshot(Cursor::new(snapshot_json)).expect("valid large snapshot");
    let summary = analyze_jsonl_with_snapshot(
        Cursor::new(concat!(
            r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#,
            "\n",
            r#"{"schema_version":1,"event_type":"session_end","session_id":"one","elapsed_ms":1}"#,
            "\n",
        )),
        &snapshot,
    )
    .expect("valid summary");

    let error = render_codex_payload_json(&summary, Some(&snapshot))
        .expect_err("large payload must fail closed");
    assert!(matches!(
        error,
        CodexPayloadError::TooLarge { bytes, maximum }
            if bytes > maximum && maximum == MAX_CODEX_PAYLOAD_BYTES
    ));
}
