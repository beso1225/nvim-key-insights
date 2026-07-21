use std::io::Cursor;

use key_insights::{ValidationErrorKind, validate_jsonl};

fn validate(input: &str) -> Result<key_insights::ValidationSummary, key_insights::ValidationError> {
    validate_jsonl(Cursor::new(input))
}

#[test]
fn validates_multiple_complete_sessions_without_buffering_the_log() {
    let input = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0,"project_id":"project-1"}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"one","elapsed_ms":10,"mode":"normal","keys":["d","d"],"duration_ms":8}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"text_run","session_id":"one","elapsed_ms":20,"key_count":4,"duration_ms":10}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_end","session_id":"one","elapsed_ms":30}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_start","session_id":"two","elapsed_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_end","session_id":"two","elapsed_ms":5}"#,
        "\n",
    );

    let summary = validate(input).expect("valid JSONL");
    assert_eq!(summary.sessions, 2);
    assert_eq!(summary.events, 6);
}

#[test]
fn rejects_insert_text_even_when_the_rest_of_the_event_is_valid() {
    let input = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"text_run","session_id":"one","elapsed_ms":5,"key_count":6,"duration_ms":5,"text":"secret"}"#,
        "\n",
    );

    let error = validate(input).expect_err("text must not be accepted");
    assert_eq!(error.line, 2);
    assert_eq!(error.kind, ValidationErrorKind::MalformedEvent);
}

#[test]
fn rejects_events_outside_a_session() {
    let input = concat!(
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"one","elapsed_ms":1,"mode":"normal","keys":["j"],"duration_ms":0}"#,
        "\n",
    );

    let error = validate(input).expect_err("session boundary is required");
    assert_eq!(error.line, 1);
    assert_eq!(error.kind, ValidationErrorKind::ExpectedSessionStart);
}

#[test]
fn rejects_session_id_changes_and_backwards_time() {
    let wrong_session = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"mode_transition","session_id":"two","elapsed_ms":1,"from":"normal","to":"insert"}"#,
        "\n",
    );
    let error = validate(wrong_session).expect_err("session ids must match");
    assert_eq!(error.line, 2);
    assert_eq!(error.kind, ValidationErrorKind::SessionMismatch);

    let backwards_time = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"one","elapsed_ms":10,"mode":"normal","keys":["j"],"duration_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_end","session_id":"one","elapsed_ms":9}"#,
        "\n",
    );
    let error = validate(backwards_time).expect_err("elapsed time must be monotonic");
    assert_eq!(error.line, 3);
    assert_eq!(error.kind, ValidationErrorKind::ElapsedTimeWentBackward);
}

#[test]
fn rejects_unsupported_versions_and_unclosed_sessions() {
    let unsupported = concat!(
        r#"{"schema_version":2,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#,
        "\n",
    );
    let error = validate(unsupported).expect_err("schema version must be supported");
    assert_eq!(error.line, 1);
    assert_eq!(
        error.kind,
        ValidationErrorKind::UnsupportedSchema { found: 2 }
    );

    let unclosed = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#,
        "\n",
    );
    let error = validate(unclosed).expect_err("session end is required");
    assert_eq!(error.line, 1);
    assert_eq!(error.kind, ValidationErrorKind::UnclosedSession);
}

#[test]
fn rejects_nested_or_reused_sessions() {
    let nested = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_start","session_id":"two","elapsed_ms":0}"#,
        "\n",
    );
    let error = validate(nested).expect_err("nested sessions are ambiguous");
    assert_eq!(error.line, 2);
    assert_eq!(error.kind, ValidationErrorKind::SessionAlreadyActive);

    let reused = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_end","session_id":"one","elapsed_ms":1}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#,
        "\n",
    );
    let error = validate(reused).expect_err("session ids must not be reused");
    assert_eq!(error.line, 3);
    assert_eq!(error.kind, ValidationErrorKind::ReusedSessionId);
}

#[test]
fn rejects_key_sequences_from_text_bearing_modes() {
    let input = concat!(
        r#"{"schema_version":1,"event_type":"session_start","session_id":"one","elapsed_ms":0}"#,
        "\n",
        r#"{"schema_version":1,"event_type":"key_sequence","session_id":"one","elapsed_ms":1,"mode":"insert","keys":["s","e","c","r","e","t"],"duration_ms":1}"#,
        "\n",
    );

    let error = validate(input).expect_err("insert sequences must not enter the schema");
    assert_eq!(error.line, 2);
    assert_eq!(error.kind, ValidationErrorKind::MalformedEvent);
}

#[test]
fn rejects_blank_lines_and_oversized_events() {
    let error = validate("\n").expect_err("blank JSONL records are invalid");
    assert_eq!(error.line, 1);
    assert_eq!(error.kind, ValidationErrorKind::MalformedEvent);

    let oversized = format!("{}\n", " ".repeat(70_000));
    let error = validate(&oversized).expect_err("event lines have a hard size limit");
    assert_eq!(error.line, 1);
    assert_eq!(error.kind, ValidationErrorKind::LineTooLong);
}
