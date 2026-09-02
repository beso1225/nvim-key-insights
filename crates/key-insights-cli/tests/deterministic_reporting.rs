use std::{
    cell::Cell,
    fs,
    io::{BufRead, BufReader, Cursor, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};

use key_insights::{
    AnalysisError, MAX_DISTINCT_ITEMS, MAX_RANKED_ITEMS, MAX_RETAINED_TOKEN_BYTES,
    MAX_SESSIONS_PER_LOG, analyze_jsonl, analyze_jsonl_inputs, render_markdown,
    render_summary_json,
};

const INPUT: &str = include_str!("fixtures/reporting.jsonl");
const EXPECTED_SUMMARY: &str = include_str!("fixtures/summary.json");
const EXPECTED_REPORT: &str = include_str!("fixtures/report.md");

#[test]
fn cli_help_and_version_are_successful_packaging_entrypoints() {
    let help = Command::new(env!("CARGO_BIN_EXE_key-insights"))
        .arg("--help")
        .output()
        .expect("run help");
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).starts_with("Usage: key-insights "));
    assert!(help.stderr.is_empty());

    let version = Command::new(env!("CARGO_BIN_EXE_key-insights"))
        .arg("--version")
        .output()
        .expect("run version");
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8_lossy(&version.stdout),
        format!("key-insights {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(version.stderr.is_empty());
}

fn repeated_motion_guard_sample(sessions: usize, sequence_keys: usize, runs: usize) -> String {
    assert!(sequence_keys >= runs * 3);
    let mut input = String::new();
    for session in 0..sessions {
        input.push_str(&format!(
            "{{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"guard-{sessions}-{sequence_keys}-{runs}-{session}\",\"elapsed_ms\":0}}\n"
        ));
        if session == 0 {
            for elapsed_ms in 1..=runs {
                input.push_str(&format!(
                    "{{\"schema_version\":1,\"event_type\":\"key_sequence\",\"session_id\":\"guard-{sessions}-{sequence_keys}-{runs}-0\",\"elapsed_ms\":{elapsed_ms},\"mode\":\"normal\",\"keys\":[\"j\",\"j\",\"j\"],\"duration_ms\":1}}\n"
                ));
            }
            let padding = vec!["x"; sequence_keys - runs * 3];
            if !padding.is_empty() {
                input.push_str(&format!(
                    "{{\"schema_version\":1,\"event_type\":\"key_sequence\",\"session_id\":\"guard-{sessions}-{sequence_keys}-{runs}-0\",\"elapsed_ms\":{},\"mode\":\"normal\",\"keys\":{},\"duration_ms\":1}}\n",
                    runs + 1,
                    serde_json::to_string(&padding).unwrap()
                ));
            }
        }
        let elapsed_ms = if session == 0 { runs + 1 } else { 0 };
        input.push_str(&format!(
            "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"guard-{sessions}-{sequence_keys}-{runs}-{session}\",\"elapsed_ms\":{elapsed_ms}}}\n"
        ));
    }
    input
}

#[test]
fn aggregates_validated_sessions_into_stable_outputs() {
    let summary = analyze_jsonl(Cursor::new(INPUT)).expect("valid analysis input");

    assert_eq!(render_summary_json(&summary), EXPECTED_SUMMARY);
    assert_eq!(render_markdown(&summary), EXPECTED_REPORT);
    assert!(!render_summary_json(&summary).contains("session_id"));
    assert!(!render_summary_json(&summary).contains("project-a"));
}

#[test]
fn summary_v3_exposes_the_versioned_ergonomic_contract() {
    let summary = analyze_jsonl(Cursor::new(INPUT)).expect("valid analysis input");
    let value = serde_json::to_value(summary).expect("summary is serializable");

    assert_eq!(value["schema_version"], 3);
    assert_eq!(value["ergonomics"]["contract_version"], 2);
    assert_eq!(value["ergonomics"]["candidate_limit"], 100);
    assert_eq!(
        value["ergonomics"]["thresholds"],
        serde_json::json!({
            "minimum_candidate_sessions": 3,
            "minimum_candidate_sequence_keys": 100,
            "minimum_candidate_observations": 3
        })
    );
    assert_eq!(value["ergonomics"]["candidates"], serde_json::json!([]));
}

#[test]
fn ergonomic_histograms_use_fixed_boundary_buckets() {
    let mut input = String::new();
    for (index, elapsed_ms) in [
        0_u64, 999, 1_000, 9_999, 10_000, 59_999, 60_000, 299_999, 300_000, 600_000,
    ]
    .into_iter()
    .enumerate()
    {
        input.push_str(&format!(
            "{{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"duration-{index}\",\"elapsed_ms\":0}}\n{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"duration-{index}\",\"elapsed_ms\":{elapsed_ms}}}\n"
        ));
    }
    let summary = analyze_jsonl(Cursor::new(input)).expect("valid duration boundaries");
    let value = serde_json::to_value(summary).expect("summary is serializable");
    assert_eq!(value["ergonomics"]["distributions"]["histogram_version"], 1);
    assert_eq!(
        value["ergonomics"]["distributions"]["session_duration_ms"],
        serde_json::json!([
            {"bucket": "0-1s", "count": 2},
            {"bucket": "1-10s", "count": 2},
            {"bucket": "10-60s", "count": 2},
            {"bucket": "1-5m", "count": 2},
            {"bucket": "over-5m", "count": 2}
        ])
    );

    let mut input = String::from(
        "{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"lengths\",\"elapsed_ms\":0}\n",
    );
    for length in [1_usize, 2, 3, 4, 5, 8, 9, 16, 17, 32, 33] {
        let keys = vec!["j"; length];
        input.push_str(&format!(
            "{{\"schema_version\":1,\"event_type\":\"key_sequence\",\"session_id\":\"lengths\",\"elapsed_ms\":0,\"mode\":\"normal\",\"keys\":{},\"duration_ms\":0}}\n",
            serde_json::to_string(&keys).unwrap()
        ));
    }
    input.push_str(
        "{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"lengths\",\"elapsed_ms\":0}\n",
    );
    let summary = analyze_jsonl(Cursor::new(input)).expect("valid length boundaries");
    let value = serde_json::to_value(summary).expect("summary is serializable");
    assert_eq!(
        value["ergonomics"]["distributions"]["sequence_length_keys"],
        serde_json::json!([
            {"bucket": "1", "count": 1},
            {"bucket": "2", "count": 1},
            {"bucket": "3-4", "count": 2},
            {"bucket": "5-8", "count": 2},
            {"bucket": "9-16", "count": 2},
            {"bucket": "17-32", "count": 2},
            {"bucket": "33-plus", "count": 1}
        ])
    );

    let mut input = String::from(
        "{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"latency\",\"elapsed_ms\":0}\n",
    );
    for duration_ms in [0_u64, 49, 50, 99, 100, 249, 250, 499, 500, 1_000] {
        input.push_str(&format!(
            "{{\"schema_version\":1,\"event_type\":\"key_sequence\",\"session_id\":\"latency\",\"elapsed_ms\":1000,\"mode\":\"normal\",\"keys\":[\"j\",\"k\"],\"duration_ms\":{duration_ms}}}\n"
        ));
    }
    for duration_ms in [199_u64, 200] {
        input.push_str(&format!(
            "{{\"schema_version\":1,\"event_type\":\"key_sequence\",\"session_id\":\"latency\",\"elapsed_ms\":1000,\"mode\":\"normal\",\"keys\":[\"j\",\"k\",\"l\"],\"duration_ms\":{duration_ms}}}\n"
        ));
    }
    input.push_str(
        "{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"latency\",\"elapsed_ms\":1000}\n",
    );
    let summary = analyze_jsonl(Cursor::new(input)).expect("valid latency boundaries");
    let value = serde_json::to_value(summary).expect("summary is serializable");
    assert_eq!(
        value["ergonomics"]["distributions"]["average_inter_key_latency_ms"],
        serde_json::json!([
            {"bucket": "0-50ms", "count": 2},
            {"bucket": "50-100ms", "count": 3},
            {"bucket": "100-250ms", "count": 3},
            {"bucket": "250-500ms", "count": 2},
            {"bucket": "over-500ms", "count": 2}
        ])
    );
}

#[test]
fn ergonomic_operations_and_mode_transitions_are_canonical_and_deterministic() {
    let input = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"operations","elapsed_ms":0}"#,
        "\n",
        r##"{"schema_version":1,"event_type":"key_sequence","session_id":"operations","elapsed_ms":9,"mode":"normal","keys":["u","<C-R>",".","/","?","n","N","*","#","x"],"duration_ms":9}"##,
        "\n",
        r#"{"schema_version":1,"event_type":"mapping_use","session_id":"operations","elapsed_ms":10,"mode":"normal","mapping_id":"mapped-u","typed_keys":["u"]}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"mode_transition","session_id":"operations","elapsed_ms":11,"from":"normal","to":"visual"}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"mode_transition","session_id":"operations","elapsed_ms":12,"from":"insert","to":"normal"}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"mode_transition","session_id":"operations","elapsed_ms":13,"from":"other","to":"command"}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_end","session_id":"operations","elapsed_ms":13}"#,
        "\n",
    );

    let value = serde_json::to_value(analyze_jsonl(Cursor::new(input)).expect("valid analysis"))
        .expect("summary is serializable");
    assert_eq!(
        value["ergonomics"]["operations"],
        serde_json::json!({
            "token_set_version": 1,
            "undo": 1,
            "redo": 1,
            "repeat": 1,
            "search_start": 2,
            "search_navigation": 4
        }),
        "mapping_use typed keys must not double-count operation evidence"
    );
    assert_eq!(
        value["ergonomics"]["mode_transitions"],
        serde_json::json!([
            {"from": "insert", "to": "normal", "count": 1},
            {"from": "normal", "to": "visual", "count": 1},
            {"from": "other", "to": "command", "count": 1}
        ])
    );
}

#[test]
fn count_prefixes_stay_within_sequences_and_preserve_zero_as_a_motion() {
    let input = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"counts","elapsed_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"counts","elapsed_ms":1,"mode":"normal","keys":["0","j"],"duration_ms":1}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"counts","elapsed_ms":2,"mode":"normal","keys":["2","j"],"duration_ms":1}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"counts","elapsed_ms":3,"mode":"visual","keys":["1","2","w"],"duration_ms":1}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"counts","elapsed_ms":4,"mode":"operator_pending","keys":["9","0","d"],"duration_ms":1}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"counts","elapsed_ms":5,"mode":"normal","keys":["3","q"],"duration_ms":1}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"counts","elapsed_ms":6,"mode":"normal","keys":["4"],"duration_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"counts","elapsed_ms":7,"mode":"normal","keys":["j"],"duration_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"counts","elapsed_ms":8,"mode":"normal","keys":["２","j"],"duration_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"mapping_use","session_id":"counts","elapsed_ms":9,"mode":"normal","mapping_id":"mapped-count","typed_keys":["7","j"]}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_end","session_id":"counts","elapsed_ms":9}"#,
        "\n",
    );

    let value = serde_json::to_value(analyze_jsonl(Cursor::new(input)).expect("valid analysis"))
        .expect("summary is serializable");
    assert_eq!(
        value["ergonomics"]["count_prefixes"],
        serde_json::json!({
            "token_set_version": 1,
            "occurrences": 3,
            "digit_presses": 5
        })
    );
}

#[test]
fn repeated_motion_candidates_require_complete_sample_guards() {
    let mut input = String::new();
    for session in 0..3 {
        input.push_str(&format!(
            "{{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"motion-{session}\",\"elapsed_ms\":0}}\n"
        ));
        input.push_str(&format!(
            "{{\"schema_version\":1,\"event_type\":\"key_sequence\",\"session_id\":\"motion-{session}\",\"elapsed_ms\":2,\"mode\":\"normal\",\"keys\":[\"j\",\"j\",\"j\"],\"duration_ms\":2}}\n"
        ));
        if session == 0 {
            let padding = vec!["x"; 91];
            input.push_str(&format!(
                "{{\"schema_version\":1,\"event_type\":\"key_sequence\",\"session_id\":\"motion-0\",\"elapsed_ms\":3,\"mode\":\"normal\",\"keys\":{},\"duration_ms\":1}}\n",
                serde_json::to_string(&padding).unwrap()
            ));
            input.push_str(
                "{\"schema_version\":1,\"event_type\":\"key_sequence\",\"session_id\":\"motion-0\",\"elapsed_ms\":4,\"mode\":\"normal\",\"keys\":[\"k\",\"k\"],\"duration_ms\":1}\n",
            );
            input.push_str(
                "{\"schema_version\":1,\"event_type\":\"key_sequence\",\"session_id\":\"motion-0\",\"elapsed_ms\":5,\"mode\":\"normal\",\"keys\":[\"k\"],\"duration_ms\":0}\n",
            );
        }
        let elapsed_ms = if session == 0 { 5 } else { 2 };
        input.push_str(&format!(
            "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"motion-{session}\",\"elapsed_ms\":{elapsed_ms}}}\n"
        ));
    }

    let summary = analyze_jsonl(Cursor::new(input)).expect("valid analysis");
    let markdown = render_markdown(&summary);
    let value = serde_json::to_value(summary).expect("summary is serializable");
    assert_eq!(
        value["ergonomics"]["repeated_motions"],
        serde_json::json!({
            "token_set_version": 1,
            "items": [{"motion": "j", "runs": 3, "presses": 9}]
        }),
        "two plus one presses in adjacent sequences must not become a run"
    );
    assert_eq!(
        value["ergonomics"]["candidates"],
        serde_json::json!([{
            "candidate_id": "repeated-motion-j",
            "kind": "repeated_motion",
            "kind_version": 1,
            "observations": 3,
            "measurements": {"presses": 9, "runs": 3},
            "guard": {
                "observed_sessions": 3,
                "observed_sequence_keys": 103,
                "required_sessions": 3,
                "required_sequence_keys": 100,
                "required_observations": 3
            }
        }])
    );
    assert!(markdown.contains(
        "| repeated-motion-j | repeated_motion v1 | 3 | presses=9, runs=3 | 3/3 sessions; 103/100 keys; 3/3 observations |"
    ));

    let short = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"short","elapsed_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"short","elapsed_ms":1,"mode":"normal","keys":["j","j","j"],"duration_ms":1}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_end","session_id":"short","elapsed_ms":1}"#,
        "\n",
    );
    let short = serde_json::to_value(analyze_jsonl(Cursor::new(short)).expect("valid analysis"))
        .expect("summary is serializable");
    assert_eq!(short["ergonomics"]["candidates"], serde_json::json!([]));

    let ranking = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"ranking","elapsed_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"ranking","elapsed_ms":1,"mode":"normal","keys":["j","j","j"],"duration_ms":1}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"ranking","elapsed_ms":2,"mode":"normal","keys":["k","k","k"],"duration_ms":1}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"ranking","elapsed_ms":3,"mode":"normal","keys":["h","h","h","h"],"duration_ms":1}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_end","session_id":"ranking","elapsed_ms":3}"#,
        "\n",
    );
    let ranking = analyze_jsonl(Cursor::new(ranking)).expect("valid ranking sample");
    assert_eq!(
        ranking
            .ergonomics
            .repeated_motions
            .items
            .iter()
            .map(|motion| (motion.motion, motion.runs, motion.presses))
            .collect::<Vec<_>>(),
        [("h", 1, 4), ("j", 1, 3), ("k", 1, 3)]
    );

    for (sessions, sequence_keys, runs, expected_candidates) in [
        (2, 100, 3, 0),
        (3, 99, 3, 0),
        (3, 100, 2, 0),
        (3, 100, 3, 1),
    ] {
        let summary = analyze_jsonl(Cursor::new(repeated_motion_guard_sample(
            sessions,
            sequence_keys,
            runs,
        )))
        .expect("valid guard boundary sample");
        assert_eq!(summary.ergonomics.candidates.len(), expected_candidates);
    }
}

#[test]
fn nonempty_candidates_do_not_reintroduce_local_session_or_project_identity() {
    let secret = "private-project-/Users/example/.env";
    let input = repeated_motion_guard_sample(3, 100, 3).replacen(
        "\"elapsed_ms\":0}",
        &format!("\"elapsed_ms\":0,\"project_id\":\"{secret}\"}}"),
        1,
    );
    let summary = analyze_jsonl(Cursor::new(input)).expect("valid guarded sample");
    assert!(!summary.ergonomics.candidates.is_empty());

    for artifact in [render_summary_json(&summary), render_markdown(&summary)] {
        assert!(!artifact.contains(secret));
        assert!(!artifact.contains("guard-3-100-3"));
    }
}

#[test]
fn aggregates_multiple_input_readers_with_global_validation_state() {
    let first = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"one","elapsed_ms":5,"mode":"normal","keys":["j"],"duration_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_end","session_id":"one","elapsed_ms":10}"#,
        "\n",
    );
    let second = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"two","elapsed_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"two","elapsed_ms":5,"mode":"normal","keys":["k"],"duration_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_end","session_id":"two","elapsed_ms":20}"#,
        "\n",
    );

    let summary = analyze_jsonl_inputs([Cursor::new(first), Cursor::new(second)])
        .expect("valid multi-input analysis");

    assert_eq!(summary.sessions, 2);
    assert_eq!(summary.events, 6);
    assert_eq!(summary.total_session_duration_ms, 30);
    assert_eq!(summary.sequence_keys, 2);
    assert_eq!(
        summary
            .keys
            .iter()
            .map(|entry| (entry.key.as_str(), entry.count))
            .collect::<Vec<_>>(),
        [("j", 1), ("k", 1)]
    );

    let combined =
        analyze_jsonl(Cursor::new(format!("{first}{second}"))).expect("valid combined analysis");
    assert_eq!(
        render_summary_json(&summary),
        render_summary_json(&combined)
    );
    assert_eq!(render_markdown(&summary), render_markdown(&combined));
}

#[test]
fn multi_input_analysis_rejects_session_reuse_in_the_later_source() {
    let session = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"same","elapsed_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_end","session_id":"same","elapsed_ms":1}"#,
        "\n",
    );

    let error = analyze_jsonl_inputs([Cursor::new(session), Cursor::new(session)])
        .expect_err("session IDs must be unique across inputs");

    assert_eq!(error.input_index, Some(1));
    assert_eq!(
        error.to_string(),
        "analysis input 2: JSONL validation failed at line 1: session ID was reused"
    );
    assert!(matches!(
        error.error,
        AnalysisError::Validation(key_insights::ValidationError {
            line: 1,
            kind: key_insights::ValidationErrorKind::ReusedSessionId,
        })
    ));
}

#[test]
fn multi_input_analysis_reports_an_unclosed_source_before_reading_the_next() {
    let unclosed = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#,
        "\n",
    );
    let valid = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"two","elapsed_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_end","session_id":"two","elapsed_ms":1}"#,
        "\n",
    );

    let error = analyze_jsonl_inputs([Cursor::new(unclosed), Cursor::new(valid)])
        .expect_err("each input must close its sessions");

    assert_eq!(error.input_index, Some(0));
    assert!(matches!(
        error.error,
        AnalysisError::Validation(key_insights::ValidationError {
            line: 1,
            kind: key_insights::ValidationErrorKind::UnclosedSession,
        })
    ));
}

#[test]
fn multi_input_analysis_rejects_an_empty_later_source() {
    let valid = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_end","session_id":"one","elapsed_ms":1}"#,
        "\n",
    );

    let error = analyze_jsonl_inputs([Cursor::new(valid), Cursor::new("")])
        .expect_err("each input must contain a complete session");

    assert_eq!(error.input_index, Some(1));
    assert_eq!(error.error, AnalysisError::NoSessions);
}

#[test]
fn multi_input_analysis_applies_the_session_limit_across_sources() {
    let inputs = (0..=MAX_SESSIONS_PER_LOG).map(|index| {
        Cursor::new(format!(
            concat!(
                "{{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"session-{}\",\"elapsed_ms\":0}}\n",
                "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"session-{}\",\"elapsed_ms\":1}}\n",
            ),
            index, index
        ))
    });

    let error = analyze_jsonl_inputs(inputs).expect_err("the session limit must be global");

    assert_eq!(error.input_index, Some(MAX_SESSIONS_PER_LOG));
    assert!(matches!(
        error.error,
        AnalysisError::Validation(key_insights::ValidationError {
            line: 1,
            kind: key_insights::ValidationErrorKind::TooManySessions,
        })
    ));
}

#[test]
fn multi_input_analysis_accepts_the_supported_session_limit_streamingly() {
    let live_inputs = Rc::new(Cell::new(0));
    let maximum_live_inputs = Rc::new(Cell::new(0));
    let inputs = (0..MAX_SESSIONS_PER_LOG).map(|index| {
        GuardedInput::new(
            format!(
                concat!(
                    "{{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"limit-{0}\",\"elapsed_ms\":0}}\n",
                    "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"limit-{0}\",\"elapsed_ms\":1}}\n",
                ),
                index
            ),
            Rc::clone(&live_inputs),
            Rc::clone(&maximum_live_inputs),
        )
    });

    let summary = analyze_jsonl_inputs(inputs).expect("the supported session limit is valid");
    let repeated = analyze_jsonl_inputs((0..MAX_SESSIONS_PER_LOG).map(|index| {
        Cursor::new(format!(
            concat!(
                "{{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"limit-{0}\",\"elapsed_ms\":0}}\n",
                "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"limit-{0}\",\"elapsed_ms\":1}}\n",
            ),
            index
        ))
    }))
    .expect("repeated supported-limit analysis");

    assert_eq!(summary.sessions, MAX_SESSIONS_PER_LOG as u64);
    assert_eq!(summary.events, (MAX_SESSIONS_PER_LOG * 2) as u64);
    assert_eq!(
        live_inputs.get(),
        0,
        "every consumed source must be dropped"
    );
    assert_eq!(
        maximum_live_inputs.get(),
        1,
        "analysis must not retain or eagerly collect input readers"
    );
    assert_eq!(
        render_summary_json(&summary),
        render_summary_json(&repeated)
    );
    assert_eq!(render_markdown(&summary), render_markdown(&repeated));
}

#[test]
fn analyzer_output_is_identical_with_single_byte_input_buffering() {
    let ordinary = analyze_jsonl(Cursor::new(INPUT)).expect("ordinary input");
    let chunked = analyze_jsonl(BufReader::with_capacity(1, Cursor::new(INPUT.as_bytes())))
        .expect("single-byte buffered input");

    assert_eq!(
        render_summary_json(&chunked),
        render_summary_json(&ordinary)
    );
    assert_eq!(render_markdown(&chunked), render_markdown(&ordinary));
}

#[test]
fn multi_input_validation_errors_take_precedence_over_earlier_analysis_errors() {
    let overflowing = format!(
        concat!(
            "{{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"one\",\"elapsed_ms\":0}}\n",
            "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"one\",\"elapsed_ms\":{}}}\n",
            "{{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"two\",\"elapsed_ms\":0}}\n",
            "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"two\",\"elapsed_ms\":1}}\n",
        ),
        u64::MAX
    );

    let error = analyze_jsonl_inputs([Cursor::new(overflowing), Cursor::new("not JSON\n".into())])
        .expect_err("all input validation must finish before reporting analysis errors");

    assert_eq!(error.input_index, Some(1));
    assert!(matches!(
        error.error,
        AnalysisError::Validation(key_insights::ValidationError {
            line: 1,
            kind: key_insights::ValidationErrorKind::MalformedEvent,
        })
    ));
}

#[test]
fn multi_input_analysis_checks_duration_overflow_across_sources() {
    let first = format!(
        concat!(
            "{{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"one\",\"elapsed_ms\":0}}\n",
            "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"one\",\"elapsed_ms\":{}}}\n",
        ),
        u64::MAX
    );
    let second = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"two","elapsed_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_end","session_id":"two","elapsed_ms":1}"#,
        "\n",
    );

    let error = analyze_jsonl_inputs([Cursor::new(first), Cursor::new(second.into())])
        .expect_err("duration limits must span every input");

    assert_eq!(error.input_index, Some(1));
    assert_eq!(error.error, AnalysisError::SessionDurationOverflow);
}

#[test]
fn cli_writes_the_same_deterministic_outputs() {
    let directory = temporary_directory("outputs");
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("input.jsonl");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&input, INPUT).expect("write input fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_key-insights"))
        .args([
            "analyze",
            path(&input),
            "--summary",
            path(&summary),
            "--report",
            path(&report),
        ])
        .output()
        .expect("run analyzer CLI");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(summary).expect("read summary"),
        EXPECTED_SUMMARY
    );
    assert_eq!(
        fs::read_to_string(report).expect("read report"),
        EXPECTED_REPORT
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn cli_accepts_a_snapshot_file_and_snapshot_stdin() {
    let directory = temporary_directory("snapshot-cli");
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("input.jsonl");
    let snapshot = directory.join("snapshot.json");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&input, INPUT).expect("write input fixture");
    let snapshot_bytes = b"{\"snapshot_version\":1,\"mappings\":[]}\n";
    fs::write(&snapshot, snapshot_bytes).expect("write snapshot");
    protect_snapshot(&snapshot);

    let output = Command::new(env!("CARGO_BIN_EXE_key-insights"))
        .args([
            "analyze",
            path(&input),
            "--summary",
            path(&summary),
            "--report",
            path(&report),
            "--keymap-snapshot",
            path(&snapshot),
        ])
        .output()
        .expect("run snapshot file analysis");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&summary).expect("read summary"))
            .expect("summary JSON");
    assert_eq!(value["schema_version"], 3);
    assert_eq!(
        value["mapping_attribution"]["mappings"][0]["status"],
        "observed_not_in_snapshot"
    );

    let stdin_summary = directory.join("stdin-summary.json");
    let stdin_report = directory.join("stdin-report.md");
    let mut child = Command::new(env!("CARGO_BIN_EXE_key-insights"))
        .args([
            "analyze",
            path(&input),
            "--summary",
            path(&stdin_summary),
            "--report",
            path(&stdin_report),
            "--keymap-snapshot",
            "-",
        ])
        .stdin(Stdio::piped())
        .spawn()
        .expect("start snapshot stdin analysis");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(snapshot_bytes)
        .expect("write snapshot stdin");
    assert!(child.wait().expect("wait for analyzer").success());
    assert_eq!(
        fs::read_to_string(&stdin_summary).expect("read stdin summary"),
        fs::read_to_string(&summary).expect("read file summary")
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn invalid_snapshot_preserves_existing_outputs() {
    let directory = temporary_directory("invalid-snapshot");
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("input.jsonl");
    let snapshot = directory.join("snapshot.json");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&input, INPUT).expect("write input fixture");
    fs::write(
        &snapshot,
        r#"{"snapshot_version":1,"mappings":[],"secret":"must-fail"}"#,
    )
    .expect("write invalid snapshot");
    protect_snapshot(&snapshot);
    fs::write(&summary, "old summary\n").expect("write old summary");
    fs::write(&report, "old report\n").expect("write old report");

    let output = Command::new(env!("CARGO_BIN_EXE_key-insights"))
        .args([
            "analyze",
            path(&input),
            "--summary",
            path(&summary),
            "--report",
            path(&report),
            "--keymap-snapshot",
            path(&snapshot),
        ])
        .output()
        .expect("run invalid snapshot analysis");

    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&summary).unwrap(), "old summary\n");
    assert_eq!(fs::read_to_string(&report).unwrap(), "old report\n");
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn cli_refuses_to_overwrite_the_snapshot_input() {
    let directory = temporary_directory("snapshot-output-alias");
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("input.jsonl");
    let snapshot = directory.join("snapshot.json");
    let report = directory.join("report.md");
    fs::write(&input, INPUT).expect("write input fixture");
    let original = "{\"snapshot_version\":1,\"mappings\":[]}\n";
    fs::write(&snapshot, original).expect("write snapshot");
    protect_snapshot(&snapshot);

    let output = Command::new(env!("CARGO_BIN_EXE_key-insights"))
        .args([
            "analyze",
            path(&input),
            "--summary",
            path(&snapshot),
            "--report",
            path(&report),
            "--keymap-snapshot",
            path(&snapshot),
        ])
        .output()
        .expect("run aliased snapshot analysis");

    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&snapshot).unwrap(), original);
    assert!(!report.exists());
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn cli_combines_multiple_positional_inputs() {
    let directory = temporary_directory("multiple-cli-inputs");
    fs::create_dir(&directory).expect("create test directory");
    let first = directory.join("first.jsonl");
    let second = directory.join("second.jsonl");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&first, session_with_key("one", "j", 10)).expect("write first input");
    fs::write(&second, session_with_key("two", "k", 20)).expect("write second input");

    let output = run_cli_inputs(&[&first, &second], &summary, &report);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&summary).expect("read summary"))
            .expect("parse summary");
    assert_eq!(value["sessions"], 2);
    assert_eq!(value["total_session_duration_ms"], 30);
    assert!(
        fs::read_to_string(report)
            .expect("read report")
            .contains("Sessions: 2")
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[cfg(unix)]
#[test]
fn cli_discovers_finalized_sessions_in_filename_order() {
    use std::os::unix::fs::PermissionsExt;

    let directory = temporary_directory("session-directory-order");
    fs::create_dir(&directory).expect("create test directory");
    let first = directory.join("nvim-key-insights-a.jsonl");
    let second = directory.join("nvim-key-insights-z.jsonl");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&second, "not JSON\n").expect("write later invalid session");
    fs::write(&first, "also not JSON\n").expect("write earlier invalid session");
    fs::set_permissions(&first, fs::Permissions::from_mode(0o600))
        .expect("make first session private");
    fs::set_permissions(&second, fs::Permissions::from_mode(0o600))
        .expect("make second session private");

    let output = run_cli_session_dir(&directory, &summary, &report);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(path(&first)), "{stderr}");
    assert!(!stderr.contains(path(&second)), "{stderr}");
    assert!(!summary.exists());
    assert!(!report.exists());
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[cfg(unix)]
#[test]
fn cli_session_directory_ignores_non_finalized_and_unsafe_entries() {
    use std::{
        ffi::CString,
        os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    };

    let directory = temporary_directory("session-directory-filtering");
    fs::create_dir(&directory).expect("create test directory");
    let finalized = directory.join("nvim-key-insights-valid.jsonl");
    fs::write(&finalized, session_with_key("valid", "j", 10)).expect("write finalized session");
    fs::set_permissions(&finalized, fs::Permissions::from_mode(0o600))
        .expect("make finalized session private");
    fs::write(
        directory.join("nvim-key-insights-partial.jsonl.part"),
        "not JSON\n",
    )
    .expect("write partial session");
    fs::write(
        directory.join("nvim-key-insights-active.lock"),
        "not JSON\n",
    )
    .expect("write lock");
    fs::write(directory.join("report.jsonl"), "not JSON\n").expect("write unrelated JSONL");
    fs::write(directory.join("legacy.jsonl"), "not JSON\n").expect("write legacy JSONL");
    let public = directory.join("nvim-key-insights-public.jsonl");
    fs::write(&public, "not JSON\n").expect("write non-private matching file");
    fs::set_permissions(&public, fs::Permissions::from_mode(0o644))
        .expect("make matching file non-private");
    let special_mode = directory.join("nvim-key-insights-special-mode.jsonl");
    fs::write(&special_mode, "not JSON\n").expect("write special-mode matching file");
    fs::set_permissions(&special_mode, fs::Permissions::from_mode(0o1600))
        .expect("set unexpected permission bits");
    fs::create_dir(directory.join("nvim-key-insights-directory.jsonl"))
        .expect("create matching directory");
    std::os::unix::fs::symlink(
        &finalized,
        directory.join("nvim-key-insights-symlink.jsonl"),
    )
    .expect("create matching symlink");
    let fifo = directory.join("nvim-key-insights-fifo.jsonl");
    let fifo = CString::new(fifo.as_os_str().as_bytes()).expect("FIFO path");
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");

    let output = run_cli_session_dir(&directory, &summary, &report);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&summary).expect("read summary"))
            .expect("parse summary");
    assert_eq!(value["sessions"], 1);
    assert_eq!(value["total_session_duration_ms"], 10);
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn cli_rejects_combining_explicit_inputs_with_a_session_directory() {
    let directory = temporary_directory("mixed-input-sources");
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("input.jsonl");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&input, session_with_key("one", "j", 10)).expect("write explicit input");

    let output = Command::new(env!("CARGO_BIN_EXE_key-insights"))
        .args([
            "analyze",
            path(&input),
            "--session-dir",
            path(&directory),
            "--summary",
            path(&summary),
            "--report",
            path(&report),
        ])
        .output()
        .expect("run analyzer CLI");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("mutually exclusive"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!summary.exists());
    assert!(!report.exists());
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn cli_reports_an_empty_session_directory_without_replacing_outputs() {
    let directory = temporary_directory("empty-session-directory");
    fs::create_dir(&directory).expect("create test directory");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&summary, "previous summary\n").expect("write previous summary");
    fs::write(&report, "previous report\n").expect("write previous report");

    let output = run_cli_session_dir(&directory, &summary, &report);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no finalized sessions found"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(&summary).expect("read previous summary"),
        "previous summary\n"
    );
    assert_eq!(
        fs::read_to_string(&report).expect("read previous report"),
        "previous report\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[cfg(unix)]
#[test]
fn cli_inputs_do_not_hold_one_descriptor_per_input() {
    use std::os::unix::fs::PermissionsExt;

    let directory = temporary_directory("bounded-session-descriptors");
    let sessions = directory.join("sessions");
    fs::create_dir_all(&sessions).expect("create session directory");
    for index in 0..96 {
        let session = sessions.join(format!("nvim-key-insights-session-{index:03}.jsonl"));
        fs::write(
            &session,
            session_with_key(&format!("session-{index:03}"), "j", 1),
        )
        .expect("write session input");
        fs::set_permissions(&session, fs::Permissions::from_mode(0o600))
            .expect("protect session input");
    }
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    let mut command = Command::new(env!("CARGO_BIN_EXE_key-insights"));
    command.args([
        "analyze",
        "--session-dir",
        path(&sessions),
        "--summary",
        path(&summary),
        "--report",
        path(&report),
    ]);
    apply_low_descriptor_limit(&mut command);

    let output = command
        .output()
        .expect("run analyzer with a low descriptor limit");
    assert!(
        output.status.success(),
        "analyzer must not retain every input descriptor: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: serde_json::Value =
        serde_json::from_slice(&fs::read(&summary).expect("read summary")).expect("parse summary");
    assert_eq!(summary["sessions"], 96);

    let explicit_summary = directory.join("explicit-summary.json");
    let explicit_report = directory.join("explicit-report.md");
    let input_paths = (0..96)
        .map(|index| sessions.join(format!("nvim-key-insights-session-{index:03}.jsonl")))
        .collect::<Vec<_>>();
    let mut explicit = Command::new(env!("CARGO_BIN_EXE_key-insights"));
    explicit.arg("analyze").args(&input_paths).args([
        "--summary",
        path(&explicit_summary),
        "--report",
        path(&explicit_report),
    ]);
    apply_low_descriptor_limit(&mut explicit);
    let explicit_output = explicit
        .output()
        .expect("run explicit analyzer inputs with a low descriptor limit");
    assert!(
        explicit_output.status.success(),
        "explicit analysis must not retain every input descriptor: {}",
        String::from_utf8_lossy(&explicit_output.stderr)
    );
    assert_eq!(
        fs::read(&explicit_summary).expect("read explicit summary"),
        fs::read(directory.join("summary.json")).expect("read discovered summary")
    );
    assert_eq!(
        fs::read(&explicit_report).expect("read explicit report"),
        fs::read(directory.join("report.md")).expect("read discovered report")
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[cfg(unix)]
#[test]
fn cli_does_not_follow_a_session_directory_symlink() {
    let root = temporary_directory("symlink-session-directory");
    let directory = root.join("sessions");
    let alias = root.join("session-alias");
    fs::create_dir_all(&directory).expect("create session directory");
    std::os::unix::fs::symlink(&directory, &alias).expect("create session directory symlink");
    let summary = root.join("summary.json");
    let report = root.join("report.md");

    let output = run_cli_session_dir(&alias, &summary, &report);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("failed to open session directory"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!summary.exists());
    assert!(!report.exists());
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn cli_rejects_duplicate_inputs_without_replacing_outputs() {
    let directory = temporary_directory("duplicate-cli-inputs");
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("input.jsonl");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&input, session_with_key("one", "j", 10)).expect("write input");
    fs::write(&summary, "previous summary\n").expect("write previous summary");
    fs::write(&report, "previous report\n").expect("write previous report");

    let output = run_cli_inputs(&[&input, &input], &summary, &report);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("duplicate input"));
    assert_eq!(
        fs::read_to_string(&summary).expect("read previous summary"),
        "previous summary\n"
    );
    assert_eq!(
        fs::read_to_string(&report).expect("read previous report"),
        "previous report\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[cfg(unix)]
#[test]
fn cli_rejects_hard_linked_duplicate_inputs() {
    let directory = temporary_directory("hard-linked-cli-inputs");
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("input.jsonl");
    let alias = directory.join("alias.jsonl");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&input, session_with_key("one", "j", 10)).expect("write input");
    fs::hard_link(&input, &alias).expect("create input hard link");

    let output = run_cli_inputs(&[&input, &alias], &summary, &report);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("duplicate input"));
    assert!(!summary.exists());
    assert!(!report.exists());
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn cli_requires_at_least_one_positional_input() {
    let directory = temporary_directory("missing-cli-input");
    fs::create_dir(&directory).expect("create test directory");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");

    let output = run_cli_inputs(&[], &summary, &report);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("<input.jsonl>..."));
    assert!(!summary.exists());
    assert!(!report.exists());
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn cli_reports_the_later_invalid_input_and_preserves_outputs() {
    let directory = temporary_directory("invalid-later-cli-input");
    fs::create_dir(&directory).expect("create test directory");
    let first = directory.join("first.jsonl");
    let invalid = directory.join("invalid.jsonl");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&first, session_with_key("one", "j", 10)).expect("write first input");
    fs::write(&invalid, "not JSON\n").expect("write invalid input");
    fs::write(&summary, "previous summary\n").expect("write previous summary");
    fs::write(&report, "previous report\n").expect("write previous report");

    let output = run_cli_inputs(&[&first, &invalid], &summary, &report);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(path(&invalid)), "{stderr}");
    assert!(stderr.contains("line 1"), "{stderr}");
    assert_eq!(
        fs::read_to_string(&summary).expect("read previous summary"),
        "previous summary\n"
    );
    assert_eq!(
        fs::read_to_string(&report).expect("read previous report"),
        "previous report\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn cli_resolves_every_input_before_touching_outputs() {
    let directory = temporary_directory("missing-later-cli-input");
    fs::create_dir(&directory).expect("create test directory");
    let first = directory.join("first.jsonl");
    let missing = directory.join("missing.jsonl");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&first, session_with_key("one", "j", 10)).expect("write first input");
    fs::write(&summary, "previous summary\n").expect("write previous summary");
    fs::write(&report, "previous report\n").expect("write previous report");

    let output = run_cli_inputs(&[&first, &missing], &summary, &report);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains(path(&missing)));
    assert_eq!(
        fs::read_to_string(&summary).expect("read previous summary"),
        "previous summary\n"
    );
    assert_eq!(
        fs::read_to_string(&report).expect("read previous report"),
        "previous report\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn cli_refuses_to_overwrite_any_input_in_a_multi_input_set() {
    let directory = temporary_directory("multi-input-output-alias");
    fs::create_dir(&directory).expect("create test directory");
    let first = directory.join("first.jsonl");
    let second = directory.join("second.jsonl");
    let report = directory.join("report.md");
    fs::write(&first, session_with_key("one", "j", 10)).expect("write first input");
    fs::write(&second, session_with_key("two", "k", 20)).expect("write second input");

    let output = run_cli_inputs(&[&first, &second], &second, &report);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("overwrite the input log"));
    assert_eq!(
        fs::read_to_string(&second).expect("read second input"),
        session_with_key("two", "k", 20)
    );
    assert!(!report.exists());
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn cli_accepts_bare_relative_paths() {
    let directory = temporary_directory("relative-paths");
    fs::create_dir(&directory).expect("create test directory");
    fs::write(directory.join("input.jsonl"), INPUT).expect("write input fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_key-insights"))
        .current_dir(&directory)
        .args([
            "analyze",
            "input.jsonl",
            "--summary",
            "summary.json",
            "--report",
            "report.md",
        ])
        .output()
        .expect("run analyzer CLI");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(directory.join("summary.json")).expect("read summary"),
        EXPECTED_SUMMARY
    );
    assert_eq!(
        fs::read_to_string(directory.join("report.md")).expect("read report"),
        EXPECTED_REPORT
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn cli_accepts_valid_long_output_file_names() {
    let directory = temporary_directory("long-output-names");
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("input.jsonl");
    let summary = directory.join(format!("{}.json", "s".repeat(240)));
    let report = directory.join(format!("{}.md", "r".repeat(240)));
    fs::write(&input, INPUT).expect("write input fixture");
    fs::write(&summary, "previous summary\n").expect("write prior summary");
    fs::write(&report, "previous report\n").expect("write prior report");

    let output = run_cli(&input, &summary, &report);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(&summary).expect("read summary"),
        EXPECTED_SUMMARY
    );
    assert_eq!(
        fs::read_to_string(&report).expect("read report"),
        EXPECTED_REPORT
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn invalid_input_does_not_create_outputs() {
    let directory = temporary_directory("invalid");
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("input.jsonl");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(
        &input,
        r#"{"schema_version":1,"event_type":"session_start","session_id":"open","elapsed_ms":0}"#,
    )
    .expect("write invalid input");

    let output = run_cli(&input, &summary, &report);

    assert!(!output.status.success());
    assert!(!summary.exists());
    assert!(!report.exists());
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn analyzer_rejects_an_empty_stream() {
    let error = analyze_jsonl(Cursor::new("")).expect_err("at least one session is required");

    assert_eq!(error, AnalysisError::NoSessions);
}

#[test]
fn analyzer_accepts_total_session_duration_at_u64_max() {
    let input = format!(
        concat!(
            "{{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"one\",\"elapsed_ms\":0}}\n",
            "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"one\",\"elapsed_ms\":{}}}\n",
            "{{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"two\",\"elapsed_ms\":0}}\n",
            "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"two\",\"elapsed_ms\":1}}\n",
        ),
        u64::MAX - 1
    );

    let summary = analyze_jsonl(Cursor::new(input)).expect("u64::MAX is an exact valid total");

    assert_eq!(summary.total_session_duration_ms, u64::MAX);
}

#[test]
fn analyzer_rejects_total_session_duration_overflow() {
    let input = format!(
        concat!(
            "{{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"one\",\"elapsed_ms\":0}}\n",
            "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"one\",\"elapsed_ms\":{}}}\n",
            "{{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"two\",\"elapsed_ms\":0}}\n",
            "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"two\",\"elapsed_ms\":1}}\n",
        ),
        u64::MAX
    );

    let error = analyze_jsonl(Cursor::new(input)).expect_err("duration overflow must fail");

    assert_eq!(error, AnalysisError::SessionDurationOverflow);
    assert_eq!(
        error.to_string(),
        "total session duration exceeds u64::MAX milliseconds"
    );
}

#[test]
fn duration_overflow_preserves_existing_output_artifacts() {
    let directory = temporary_directory("duration-overflow");
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("input.jsonl");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    let overflowing_input = format!(
        concat!(
            "{{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"one\",\"elapsed_ms\":0}}\n",
            "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"one\",\"elapsed_ms\":{}}}\n",
            "{{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"two\",\"elapsed_ms\":0}}\n",
            "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"two\",\"elapsed_ms\":1}}\n",
        ),
        u64::MAX
    );
    fs::write(&input, overflowing_input).expect("write overflowing input");
    fs::write(&summary, "previous summary\n").expect("write prior summary");
    fs::write(&report, "previous report\n").expect("write prior report");

    let output = run_cli(&input, &summary, &report);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("total session duration exceeds u64::MAX milliseconds")
    );
    assert_eq!(
        fs::read_to_string(&summary).expect("read prior summary"),
        "previous summary\n"
    );
    assert_eq!(
        fs::read_to_string(&report).expect("read prior report"),
        "previous report\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn cli_rejects_incomplete_collector_artifacts() {
    let directory = temporary_directory("incomplete-input");
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("nvim-key-insights-session.jsonl.part");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&input, INPUT).expect("write complete but unpublished input");

    let output = run_cli(&input, &summary, &report);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("incomplete collector artifact"));
    assert!(!summary.exists());
    assert!(!report.exists());
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn report_escapes_untrusted_key_tokens() {
    let input = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"one","elapsed_ms":1,"mode":"normal","keys":["|<script>\n"],"duration_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_end","session_id":"one","elapsed_ms":2}"#,
        "\n",
    );
    let summary = analyze_jsonl(Cursor::new(input)).expect("valid input");
    let report = render_markdown(&summary);

    assert!(report.contains("<code>&#124;&lt;script&gt;\\n</code>"));
    assert!(!report.contains("<script>"));
}

#[test]
fn report_escapes_terminal_control_characters_in_tokens() {
    let input = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"one","elapsed_ms":1,"mode":"normal","keys":["\u001b[31m","\t"],"duration_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"mapping_use","session_id":"one","elapsed_ms":2,"mode":"normal","mapping_id":"map-\u0000","typed_keys":["g"]}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_end","session_id":"one","elapsed_ms":3}"#,
        "\n",
    );
    let summary = analyze_jsonl(Cursor::new(input)).expect("valid input");
    let report = render_markdown(&summary);

    assert!(report.contains(r"<code>\u{1b}[31m</code>"));
    assert!(report.contains(r"<code>\t</code>"));
    assert!(report.contains(r"<code>map-\u{0}</code>"));
    assert!(!report.contains('\u{001b}'));
    assert!(!report.contains('\t'));
    assert!(!report.contains('\0'));
}

#[test]
fn report_escapes_unicode_format_and_separator_characters_in_tokens() {
    let format_controls = "\u{202e}\u{2066}";
    let separators = "\u{2028}\u{2029}";
    let key_event = serde_json::json!({
        "schema_version": 1,
        "event_type": "key_sequence",
        "session_id": "one",
        "elapsed_ms": 1,
        "mode": "normal",
        "keys": [format!("key-{format_controls}")],
        "duration_ms": 0
    });
    let mapping_event = serde_json::json!({
        "schema_version": 1,
        "event_type": "mapping_use",
        "session_id": "one",
        "elapsed_ms": 2,
        "mode": "normal",
        "mapping_id": format!("map-{separators}"),
        "typed_keys": ["g"]
    });
    let input = format!(
        "{}\n{key_event}\n{mapping_event}\n{}\n",
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#,
        r#"{"schema_version":1,"event_type":"session_end","session_id":"one","elapsed_ms":3}"#
    );

    let summary = analyze_jsonl(Cursor::new(input)).expect("valid input");
    let report = render_markdown(&summary);

    assert!(report.contains(r"<code>key-\u{202e}\u{2066}</code>"));
    assert!(report.contains(r"<code>map-\u{2028}\u{2029}</code>"));
    for character in format_controls.chars().chain(separators.chars()) {
        assert!(!report.contains(character));
    }
}

#[test]
fn cli_refuses_to_overwrite_the_input_log() {
    let directory = temporary_directory("overwrite");
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("input.jsonl");
    let report = directory.join("report.md");
    fs::write(&input, INPUT).expect("write input");

    let output = run_cli(&input, &input, &report);

    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&input).expect("input survives"), INPUT);
    assert!(!report.exists());
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn summary_bounds_ranked_tokens_and_reports_total_cardinality() {
    let mut input =
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#
            .to_owned();
    input.push('\n');
    for index in 0..=MAX_RANKED_ITEMS {
        input.push_str(&format!(
            "{{\"schema_version\":1,\"event_type\":\"key_sequence\",\"session_id\":\"one\",\"elapsed_ms\":{},\"mode\":\"normal\",\"keys\":[\"key-{index:03}\"],\"duration_ms\":0}}\n",
            index + 1
        ));
    }
    input.push_str(&format!(
        "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"one\",\"elapsed_ms\":{}}}\n",
        MAX_RANKED_ITEMS + 2
    ));

    let summary = analyze_jsonl(Cursor::new(input)).expect("valid input");

    assert_eq!(summary.ranking_limit, MAX_RANKED_ITEMS);
    assert_eq!(summary.unique_keys, MAX_RANKED_ITEMS as u64 + 1);
    assert_eq!(summary.keys.len(), MAX_RANKED_ITEMS);
    assert_eq!(summary.keys.first().expect("first key").key, "key-000");
    assert_eq!(
        summary.keys.last().expect("last ranked key").key,
        format!("key-{:03}", MAX_RANKED_ITEMS - 1)
    );
}

#[test]
fn cli_refuses_normalized_aliases_of_the_input_log() {
    let directory = temporary_directory("normalized-overwrite");
    let nested = directory.join("nested");
    fs::create_dir_all(&nested).expect("create test directory");
    let input = directory.join("input.jsonl");
    let aliased_input = nested.join("..").join("input.jsonl");
    let report = directory.join("report.md");
    fs::write(&input, INPUT).expect("write input");

    let output = run_cli(&input, &aliased_input, &report);

    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&input).expect("input survives"), INPUT);
    assert!(!report.exists());
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn analyzer_rejects_unbounded_distinct_key_cardinality() {
    let mut input =
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#
            .to_owned();
    input.push('\n');
    for index in 0..=MAX_DISTINCT_ITEMS {
        input.push_str(&format!(
            "{{\"schema_version\":1,\"event_type\":\"key_sequence\",\"session_id\":\"one\",\"elapsed_ms\":{},\"mode\":\"normal\",\"keys\":[\"key-{index:05}\"],\"duration_ms\":0}}\n",
            index + 1
        ));
    }
    input.push_str(&format!(
        "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"one\",\"elapsed_ms\":{}}}\n",
        MAX_DISTINCT_ITEMS + 2
    ));

    let error = analyze_jsonl(Cursor::new(input)).expect_err("cardinality must be bounded");

    assert!(error.to_string().contains("distinct key limit"));
}

#[test]
fn analyzer_rejects_unbounded_distinct_mapping_cardinality() {
    let mut input =
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#
            .to_owned();
    input.push('\n');
    for index in 0..=MAX_DISTINCT_ITEMS {
        input.push_str(&format!(
            "{{\"schema_version\":1,\"event_type\":\"mapping_use\",\"session_id\":\"one\",\"elapsed_ms\":{},\"mode\":\"normal\",\"mapping_id\":\"map-{index:05}\",\"typed_keys\":[\"g\"]}}\n",
            index + 1
        ));
    }
    input.push_str(&format!(
        "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"one\",\"elapsed_ms\":{}}}\n",
        MAX_DISTINCT_ITEMS + 2
    ));

    let error = analyze_jsonl(Cursor::new(input)).expect_err("cardinality must be bounded");

    assert!(error.to_string().contains("distinct mapping limit"));
}

#[test]
fn output_setup_failure_preserves_existing_artifacts() {
    let directory = temporary_directory("atomic-output");
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("input.jsonl");
    let summary = directory.join("summary.json");
    let report_directory = directory.join("report-directory");
    fs::write(&input, INPUT).expect("write input");
    fs::write(&summary, "previous summary\n").expect("write prior summary");
    fs::create_dir(&report_directory).expect("create invalid report destination");

    let output = run_cli(&input, &summary, &report_directory);

    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(&summary).expect("summary survives"),
        "previous summary\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[cfg(unix)]
#[test]
fn cli_refuses_dangling_output_symlinks() {
    use std::os::unix::fs::symlink;

    let directory = temporary_directory("dangling-symlink");
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("input.jsonl");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&input, INPUT).expect("write input");
    symlink(&report, &summary).expect("create dangling output symlink");

    let output = run_cli(&input, &summary, &report);

    assert!(!output.status.success());
    assert!(
        fs::symlink_metadata(&summary)
            .expect("symlink survives")
            .file_type()
            .is_symlink()
    );
    assert!(!report.exists());
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[cfg(unix)]
#[test]
fn cli_refuses_existing_non_regular_output_files() {
    use std::os::unix::fs::FileTypeExt;

    let directory = temporary_directory("special-output");
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("input.jsonl");
    let summary_fifo = directory.join("summary.fifo");
    let report = directory.join("report.md");
    fs::write(&input, INPUT).expect("write input");
    assert!(
        Command::new("mkfifo")
            .arg(&summary_fifo)
            .status()
            .expect("run mkfifo")
            .success(),
        "create output FIFO"
    );

    let output = run_cli(&input, &summary_fifo, &report);

    assert!(!output.status.success());
    assert!(
        fs::symlink_metadata(&summary_fifo)
            .expect("FIFO survives")
            .file_type()
            .is_fifo()
    );
    assert!(!report.exists());
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn analyzer_bounds_total_retained_token_bytes_across_categories() {
    let token_size = 60 * 1024;
    let token_count = MAX_RETAINED_TOKEN_BYTES / token_size + 2;
    let mut input =
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#
            .to_owned();
    input.push('\n');
    for index in 0..token_count {
        let token = format!("{index:04}-{}", "x".repeat(token_size - 5));
        let event = if index % 2 == 0 {
            serde_json::json!({
                "schema_version": 1,
                "event_type": "key_sequence",
                "session_id": "one",
                "elapsed_ms": index + 1,
                "mode": "normal",
                "keys": [token],
                "duration_ms": 0
            })
        } else {
            serde_json::json!({
                "schema_version": 1,
                "event_type": "mapping_use",
                "session_id": "one",
                "elapsed_ms": index + 1,
                "mode": "normal",
                "mapping_id": token,
                "typed_keys": ["g"]
            })
        };
        input.push_str(&event.to_string());
        input.push('\n');
    }
    input.push_str(&format!(
        "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"one\",\"elapsed_ms\":{}}}\n",
        token_count + 1
    ));

    let error = analyze_jsonl(Cursor::new(input)).expect_err("retained bytes must be bounded");

    assert_eq!(error, AnalysisError::RetainedTokenBytesExceeded);
}

#[test]
fn analyzer_accepts_long_schema_v1_tokens_within_the_event_limit() {
    let long_key = format!("key-{}", "k".repeat(60 * 1024));
    let long_mapping = format!("map-{}", "m".repeat(60 * 1024));
    let key_event = serde_json::json!({
        "schema_version": 1,
        "event_type": "key_sequence",
        "session_id": "one",
        "elapsed_ms": 1,
        "mode": "normal",
        "keys": [long_key],
        "duration_ms": 0
    });
    let mapping_event = serde_json::json!({
        "schema_version": 1,
        "event_type": "mapping_use",
        "session_id": "one",
        "elapsed_ms": 2,
        "mode": "normal",
        "mapping_id": long_mapping,
        "typed_keys": ["g"]
    });
    let input = format!(
        "{}\n{key_event}\n{mapping_event}\n{}\n",
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#,
        r#"{"schema_version":1,"event_type":"session_end","session_id":"one","elapsed_ms":3}"#
    );

    let summary = analyze_jsonl(Cursor::new(input)).expect("schema-v1 long tokens remain valid");

    assert_eq!(summary.unique_keys, 1);
    assert_eq!(summary.unique_mappings, 1);
}

#[test]
fn cli_rejects_absent_output_names_that_differ_only_by_ascii_case() {
    let directory = temporary_directory("case-collision");
    fs::create_dir(&directory).expect("create test directory");
    let upper_probe = directory.join("CaseProbe");
    let lower_probe = directory.join("caseprobe");
    fs::write(&upper_probe, "probe").expect("write case probe");
    let case_insensitive = lower_probe.exists();
    fs::remove_file(&upper_probe).expect("remove case probe");

    let input = directory.join("input.jsonl");
    let summary = directory.join("result");
    let report = directory.join("RESULT");
    fs::write(&input, INPUT).expect("write input");

    let output = run_cli(&input, &summary, &report);

    assert_eq!(
        output.status.success(),
        !case_insensitive,
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    if case_insensitive {
        assert!(!summary.exists());
    } else {
        assert_eq!(
            fs::read_to_string(&summary).expect("read distinct summary"),
            EXPECTED_SUMMARY
        );
        assert_eq!(
            fs::read_to_string(&report).expect("read distinct report"),
            EXPECTED_REPORT
        );
    }
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn output_alias_detection_follows_filesystem_unicode_normalization() {
    let directory = temporary_directory("unicode-collision");
    fs::create_dir(&directory).expect("create test directory");
    let composed_probe = directory.join("probe-\u{00e9}");
    let decomposed_probe = directory.join("probe-e\u{0301}");
    fs::write(&composed_probe, "probe").expect("write normalization probe");
    let normalization_insensitive = decomposed_probe.exists();
    fs::remove_file(&composed_probe).expect("remove normalization probe");

    let input = directory.join("input.jsonl");
    let summary = directory.join("result-\u{00e9}");
    let report = directory.join("result-e\u{0301}");
    fs::write(&input, INPUT).expect("write input");

    let output = run_cli(&input, &summary, &report);

    assert_eq!(
        output.status.success(),
        !normalization_insensitive,
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    if normalization_insensitive {
        assert!(!summary.exists());
    } else {
        assert_eq!(
            fs::read_to_string(&summary).expect("read distinct summary"),
            EXPECTED_SUMMARY
        );
        assert_eq!(
            fs::read_to_string(&report).expect("read distinct report"),
            EXPECTED_REPORT
        );
    }
    fs::remove_dir_all(directory).expect("remove test directory");
}

fn run_cli(input: &Path, summary: &Path, report: &Path) -> std::process::Output {
    run_cli_inputs(&[input], summary, report)
}

fn run_cli_inputs(inputs: &[&Path], summary: &Path, report: &Path) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_key-insights"));
    command.arg("analyze");
    for input in inputs {
        command.arg(input);
    }
    command
        .args(["--summary", path(summary), "--report", path(report)])
        .output()
        .expect("run analyzer CLI")
}

fn run_cli_session_dir(
    session_directory: &Path,
    summary: &Path,
    report: &Path,
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_key-insights"))
        .args([
            "analyze",
            "--session-dir",
            path(session_directory),
            "--summary",
            path(summary),
            "--report",
            path(report),
        ])
        .output()
        .expect("run analyzer CLI")
}

fn session_with_key(session_id: &str, key: &str, duration_ms: u64) -> String {
    format!(
        concat!(
            "{{\"schema_version\":1,\"event_type\":\"session_start\",\"session_id\":\"{}\",\"elapsed_ms\":0}}\n",
            "{{\"schema_version\":1,\"event_type\":\"key_sequence\",\"session_id\":\"{}\",\"elapsed_ms\":1,\"mode\":\"normal\",\"keys\":[\"{}\"],\"duration_ms\":0}}\n",
            "{{\"schema_version\":1,\"event_type\":\"session_end\",\"session_id\":\"{}\",\"elapsed_ms\":{}}}\n",
        ),
        session_id, session_id, key, session_id, duration_ms
    )
}

fn temporary_directory(label: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "key-insights-{label}-{}-{unique}",
        std::process::id()
    ))
}

fn path(path: &Path) -> &str {
    path.to_str().expect("UTF-8 test path")
}

#[cfg(unix)]
fn apply_low_descriptor_limit(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    unsafe {
        command.pre_exec(|| {
            let mut limit = std::mem::zeroed::<libc::rlimit>();
            if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            limit.rlim_cur = limit.rlim_cur.min(32);
            if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
}

struct GuardedInput {
    cursor: Cursor<Vec<u8>>,
    live: Rc<Cell<usize>>,
}

impl GuardedInput {
    fn new(contents: String, live: Rc<Cell<usize>>, maximum_live: Rc<Cell<usize>>) -> Self {
        let next = live.get() + 1;
        live.set(next);
        maximum_live.set(maximum_live.get().max(next));
        Self {
            cursor: Cursor::new(contents.into_bytes()),
            live,
        }
    }
}

impl Drop for GuardedInput {
    fn drop(&mut self) {
        self.live.set(self.live.get() - 1);
    }
}

impl Read for GuardedInput {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.cursor.read(buffer)
    }
}

impl BufRead for GuardedInput {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.cursor.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        self.cursor.consume(amount);
    }
}

#[cfg(unix)]
fn protect_snapshot(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("protect snapshot");
}

#[cfg(not(unix))]
fn protect_snapshot(_path: &Path) {}
