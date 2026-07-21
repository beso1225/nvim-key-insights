use key_insights::{PrivacyPolicy, SCHEMA_VERSION};

#[test]
fn schema_starts_at_version_one() {
    assert_eq!(SCHEMA_VERSION, 1);
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
