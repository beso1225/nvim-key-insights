use std::io::Cursor;

use key_insights::{
    AnalysisError, MappingAttributionStatus, analyze_jsonl_with_snapshot, parse_keymap_snapshot,
};
use sha2::{Digest, Sha256};

const GLOBAL_GG: &str =
    "mapping-v1:a27261baf28b456378725590385ed469ee8c2c2e3fd5173cd32c7dbec271cc71";
const BUFFER_GG: &str =
    "mapping-v1:302f7c048154b3a3aaaf2ee95ef609842b71dd44884239709e9a77eea72c7757";
const VISUAL_X: &str =
    "mapping-v1:27b6579d750b841290c70bfb190c9879147a32505f8348dcf478dffd671123f1";

fn snapshot(mappings: &str) -> String {
    format!(r#"{{"snapshot_version":1,"mappings":[{mappings}]}}"#)
}

fn mapping(id: &str, mode: &str, scope: &str, lhs: &str) -> String {
    format!(r#"{{"mapping_id":"{id}","mode":"{mode}","scope":"{scope}","lhs":{lhs}}}"#)
}

#[test]
fn parses_a_strict_canonical_snapshot() {
    let input = snapshot(
        &[
            mapping(BUFFER_GG, "normal", "buffer", r#"["g","g"]"#),
            mapping(GLOBAL_GG, "normal", "global", r#"["g","g"]"#),
            mapping(VISUAL_X, "visual", "global", r#"["x"]"#),
        ]
        .join(","),
    );

    let parsed = parse_keymap_snapshot(Cursor::new(input)).expect("valid snapshot");

    assert_eq!(parsed.mappings.len(), 3);
}

#[test]
fn rejects_unknown_fields_versions_duplicates_invalid_ids_tokens_and_order() {
    let cases = [
        r#"{"snapshot_version":1,"mappings":[],"path":"/secret"}"#.to_owned(),
        r#"{"snapshot_version":2,"mappings":[]}"#.to_owned(),
        snapshot(
            &[
                mapping(GLOBAL_GG, "normal", "global", r#"["g","g"]"#),
                mapping(GLOBAL_GG, "normal", "global", r#"["g","g"]"#),
            ]
            .join(","),
        ),
        snapshot(&mapping(
            "mapping-v1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "normal",
            "global",
            r#"["g","g"]"#,
        )),
        snapshot(&mapping(GLOBAL_GG, "normal", "global", r#"["<C-x"]"#)),
        snapshot(
            &[
                mapping(VISUAL_X, "visual", "global", r#"["x"]"#),
                mapping(GLOBAL_GG, "normal", "global", r#"["g","g"]"#),
            ]
            .join(","),
        ),
    ];

    for input in cases {
        assert!(
            parse_keymap_snapshot(Cursor::new(input)).is_err(),
            "invalid snapshot was accepted"
        );
    }
}

#[test]
fn rejects_snapshot_resource_limit_violations() {
    let too_many_tokens = vec![r#""x""#; 65].join(",");
    let input = snapshot(&mapping(
        GLOBAL_GG,
        "normal",
        "global",
        &format!("[{too_many_tokens}]"),
    ));
    assert!(parse_keymap_snapshot(Cursor::new(input)).is_err());

    let oversized = vec![b' '; 1024 * 1024 + 1];
    assert!(parse_keymap_snapshot(Cursor::new(oversized)).is_err());

    let entry = mapping(GLOBAL_GG, "normal", "global", r#"["g","g"]"#);
    let too_many_mappings = snapshot(&vec![entry; 4097].join(","));
    assert!(parse_keymap_snapshot(Cursor::new(too_many_mappings)).is_err());
}

#[test]
fn joins_observed_missing_unobserved_and_collisions_deterministically() {
    let log = format!(
        concat!(
            "{{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"one\",\"elapsed_ms\":0}}\n",
            "{{\"schema_version\":1,\"event_type\":\"mapping_use\",\"session_id\":\"one\",\"elapsed_ms\":1,\"mode\":\"normal\",\"mapping_id\":\"{}\",\"typed_keys\":[\"g\",\"g\"]}}\n",
            "{{\"schema_version\":1,\"event_type\":\"mapping_use\",\"session_id\":\"one\",\"elapsed_ms\":2,\"mode\":\"normal\",\"mapping_id\":\"mapping-v1:removed\",\"typed_keys\":[\"z\"]}}\n",
            "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"one\",\"elapsed_ms\":3}}\n"
        ),
        GLOBAL_GG
    );
    let snapshot = parse_keymap_snapshot(Cursor::new(snapshot(
        &[
            mapping(BUFFER_GG, "normal", "buffer", r#"["g","g"]"#),
            mapping(GLOBAL_GG, "normal", "global", r#"["g","g"]"#),
            mapping(VISUAL_X, "visual", "global", r#"["x"]"#),
        ]
        .join(","),
    )))
    .expect("valid snapshot");

    let summary = analyze_jsonl_with_snapshot(Cursor::new(log), &snapshot).expect("valid analysis");
    let attribution = summary.mapping_attribution.expect("snapshot attribution");

    assert_eq!(summary.schema_version, 3);
    assert_eq!(attribution.snapshot_version, 1);
    assert_eq!(attribution.mappings.len(), 4);
    assert_eq!(attribution.mappings[0].mapping_id, GLOBAL_GG);
    assert_eq!(
        attribution.mappings[0].status,
        MappingAttributionStatus::Observed
    );
    assert_eq!(attribution.mappings[0].count, 1);
    assert_eq!(attribution.mappings[0].collision_mapping_ids, [BUFFER_GG]);
    assert_eq!(
        attribution.mappings[1].status,
        MappingAttributionStatus::ObservedNotInSnapshot
    );
    assert_eq!(
        attribution.mappings[2].status,
        MappingAttributionStatus::UnobservedInSample
    );
    assert_eq!(attribution.mappings[2].collision_mapping_ids, [GLOBAL_GG]);
    assert_eq!(
        attribution.mappings[3].status,
        MappingAttributionStatus::UnobservedInSample
    );
}

#[test]
fn rejects_snapshot_event_mode_or_lhs_conflicts() {
    let snapshot = parse_keymap_snapshot(Cursor::new(snapshot(&mapping(
        GLOBAL_GG,
        "normal",
        "global",
        r#"["g","g"]"#,
    ))))
    .expect("valid snapshot");
    let conflicting = format!(
        concat!(
            "{{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"one\",\"elapsed_ms\":0}}\n",
            "{{\"schema_version\":1,\"event_type\":\"mapping_use\",\"session_id\":\"one\",\"elapsed_ms\":1,\"mode\":\"normal\",\"mapping_id\":\"{}\",\"typed_keys\":[\"x\"]}}\n",
            "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"one\",\"elapsed_ms\":2}}\n"
        ),
        GLOBAL_GG
    );

    let error = analyze_jsonl_with_snapshot(Cursor::new(conflicting), &snapshot)
        .expect_err("snapshot/event conflicts must fail closed");
    assert_eq!(error, AnalysisError::SnapshotEventMismatch);
}

#[test]
fn snapshot_join_keeps_all_bindings_before_the_top_100_ranking() {
    let mut mappings = Vec::new();
    let mut events = String::from(
        "{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"one\",\"elapsed_ms\":0}\n",
    );
    for index in 0..101 {
        let token = char::from_u32(0x100 + index).unwrap().to_string();
        let id = mapping_id("normal", "global", &[token.as_str()]);
        mappings.push(mapping(&id, "normal", "global", &format!("[\"{token}\"]")));
        events.push_str(&format!(
            "{{\"schema_version\":1,\"event_type\":\"mapping_use\",\"session_id\":\"one\",\"elapsed_ms\":{},\"mode\":\"normal\",\"mapping_id\":\"{}\",\"typed_keys\":[\"{}\"]}}\n",
            index + 1,
            id,
            token
        ));
    }
    events.push_str(
        "{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"one\",\"elapsed_ms\":102}\n",
    );
    let snapshot = parse_keymap_snapshot(Cursor::new(snapshot(&mappings.join(","))))
        .expect("valid large snapshot");

    let summary = analyze_jsonl_with_snapshot(Cursor::new(events), &snapshot)
        .expect("valid snapshot-aware analysis");

    assert_eq!(summary.mappings.len(), 100);
    assert_eq!(summary.mapping_attribution.unwrap().mappings.len(), 101);
}

fn mapping_id(mode: &str, scope: &str, lhs: &[&str]) -> String {
    let mut preimage = String::new();
    let count = lhs.len().to_string();
    for value in ["mapping-v1", mode, scope, count.as_str()] {
        preimage.push_str(&format!("{}:{value}", value.len()));
    }
    for token in lhs {
        preimage.push_str(&format!("{}:{token}", token.len()));
    }
    format!("mapping-v1:{:x}", Sha256::digest(preimage.as_bytes()))
}
