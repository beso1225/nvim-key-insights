use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use key_insights::{
    MAX_DISTINCT_ITEMS, MAX_RANKED_ITEMS, analyze_jsonl, render_markdown, render_summary_json,
};

const INPUT: &str = include_str!("fixtures/reporting.jsonl");
const EXPECTED_SUMMARY: &str = include_str!("fixtures/summary.json");
const EXPECTED_REPORT: &str = include_str!("fixtures/report.md");

#[test]
fn aggregates_validated_sessions_into_stable_outputs() {
    let summary = analyze_jsonl(Cursor::new(INPUT)).expect("valid analysis input");

    assert_eq!(render_summary_json(&summary), EXPECTED_SUMMARY);
    assert_eq!(render_markdown(&summary), EXPECTED_REPORT);
    assert!(!render_summary_json(&summary).contains("session_id"));
    assert!(!render_summary_json(&summary).contains("project-a"));
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

fn run_cli(input: &Path, summary: &Path, report: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_key-insights"))
        .args([
            "analyze",
            path(input),
            "--summary",
            path(summary),
            "--report",
            path(report),
        ])
        .output()
        .expect("run analyzer CLI")
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
