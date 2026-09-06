use key_insights::{PrivacyPolicy, SCHEMA_VERSION};

#[test]
fn schema_uses_the_current_event_log_version() {
    assert_eq!(SCHEMA_VERSION, 2);
}

#[test]
fn privacy_sensitive_capture_is_disabled_by_default() {
    let policy = PrivacyPolicy::default();

    assert!(!policy.raw_keylog);
    assert!(!policy.capture_insert_text);
    assert!(!policy.capture_command_text);
    assert!(!policy.capture_search_text);
    assert!(!policy.store_file_paths);
}
