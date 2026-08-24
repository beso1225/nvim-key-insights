#![cfg(unix)]

use std::{
    ffi::OsString,
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use key_insights::{
    CodexExecConfig, CodexExecError, MAX_CODEX_OUTPUT_BYTES, build_codex_exec_argv, run_codex_exec,
};

fn temp_script(name: &str, body: &str) -> (PathBuf, PathBuf) {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("key-insights-codex-{name}-{unique}"));
    fs::create_dir(&directory).expect("create test directory");
    let script = directory.join("mock-codex");
    fs::write(&script, format!("#!/bin/sh\n{body}\n")).expect("write mock executable");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).expect("make mock executable");
    (directory, script)
}

fn config(binary: &Path) -> CodexExecConfig {
    let working_directory = binary
        .parent()
        .expect("binary parent")
        .join("empty-workspace");
    if binary.exists() {
        fs::create_dir_all(&working_directory).expect("create empty workspace");
        fs::set_permissions(&working_directory, fs::Permissions::from_mode(0o700))
            .expect("protect empty workspace");
        let codex_home = binary.parent().expect("binary parent").join("codex-home");
        fs::create_dir_all(&codex_home).expect("create Codex home");
        fs::set_permissions(&codex_home, fs::Permissions::from_mode(0o700))
            .expect("protect Codex home");
    }
    CodexExecConfig {
        binary: binary.to_owned(),
        codex_home: binary.parent().expect("binary parent").join("codex-home"),
        output_schema: PathBuf::from("/private/schema with spaces.json"),
        path_environment: OsString::from("/usr/bin:/bin"),
        working_directory,
        timeout: Duration::from_secs(2),
        max_output_bytes: MAX_CODEX_OUTPUT_BYTES,
    }
}

#[test]
fn passes_only_the_explicit_codex_environment() {
    let (directory, script) = temp_script(
        "environment",
        r#"cat >/dev/null
test -z "${HOME+x}" || exit 20
test -z "${OPENAI_API_KEY+x}" || exit 21
test -z "${KEY_INSIGHTS_UNRELATED_ENV_CANARY+x}" || exit 22
test -z "${HTTPS_PROXY+x}" || exit 23
test -z "${SSL_CERT_FILE+x}" || exit 24
test -z "${DBUS_SESSION_BUS_ADDRESS+x}" || exit 25
printf '%s\n%s' "$CODEX_HOME" "$PATH""#,
    );
    let options = config(&script);
    let expected = format!("{}\n/usr/bin:/bin", options.codex_home.display());
    let result = run_codex_exec(&options, b"payload").expect("isolated environment succeeds");
    assert_eq!(result.stdout, expected.as_bytes());
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn builds_a_non_shell_codex_exec_argv_with_privacy_flags() {
    let argv = build_codex_exec_argv(&config(Path::new("/private/codex with spaces")));
    assert_eq!(
        argv,
        vec![
            "exec",
            "--ephemeral",
            "--ignore-user-config",
            "--ignore-rules",
            "--strict-config",
            "--skip-git-repo-check",
            "--cd",
            "/private/empty-workspace",
            "--config",
            "shell_environment_policy.inherit=\"none\"",
            "--config",
            "approval_policy=\"never\"",
            "--config",
            "default_permissions=\"key-insights-payload-only\"",
            "--config",
            "permissions.key-insights-payload-only.filesystem={\":root\"=\"deny\",\":minimal\"=\"read\"}",
            "--config",
            "permissions.key-insights-payload-only.network.enabled=false",
            "--output-schema",
            "/private/schema with spaces.json",
        ]
    );
}

#[test]
fn runs_a_mocked_codex_process_with_payload_on_stdin() {
    let (directory, script) = temp_script("success", "cat >/dev/null; printf '{\"ok\":true}'");
    let result = run_codex_exec(&config(&script), br#"{"summary":"sanitized"}"#)
        .expect("mock Codex succeeds");
    assert_eq!(result.stdout, br#"{"ok":true}"#);
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn rejects_a_caller_supplied_output_limit_above_the_global_bound() {
    let (directory, script) = temp_script("invalid-limit", "cat >/dev/null");
    let mut options = config(&script);
    options.max_output_bytes = MAX_CODEX_OUTPUT_BYTES + 1;
    assert_eq!(
        run_codex_exec(&options, b"payload").expect_err("limit must be rejected"),
        CodexExecError::InvalidConfig
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn rejects_missing_explicit_codex_environment_values() {
    let (directory, script) = temp_script("invalid-environment", "cat >/dev/null");
    let mut options = config(&script);
    options.codex_home = PathBuf::from("relative-codex-home");
    assert_eq!(
        run_codex_exec(&options, b"payload").expect_err("relative home must fail"),
        CodexExecError::InvalidConfig
    );
    options.codex_home = directory.join("codex-home");
    fs::create_dir_all(&options.codex_home).expect("create valid Codex home");
    options.path_environment = OsString::new();
    assert_eq!(
        run_codex_exec(&options, b"payload").expect_err("empty PATH must fail"),
        CodexExecError::InvalidConfig
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn rejects_missing_codex_home_and_relative_binary() {
    let (directory, script) = temp_script("missing-environment", "cat >/dev/null");
    let mut options = config(&script);
    options.codex_home = directory.join("missing-codex-home");
    assert_eq!(
        run_codex_exec(&options, b"payload").expect_err("missing home must fail"),
        CodexExecError::InvalidConfig
    );
    options.codex_home = directory.join("codex-home");
    options.binary = PathBuf::from("codex");
    assert_eq!(
        run_codex_exec(&options, b"payload").expect_err("relative binary must fail"),
        CodexExecError::InvalidConfig
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn rejects_a_symlinked_codex_home() {
    let (directory, script) = temp_script("symlinked-home", "cat >/dev/null");
    let real_home = directory.join("real-codex-home");
    let linked_home = directory.join("linked-codex-home");
    fs::create_dir(&real_home).expect("create real Codex home");
    symlink(&real_home, &linked_home).expect("link Codex home");
    let mut options = config(&script);
    options.codex_home = linked_home;
    assert_eq!(
        run_codex_exec(&options, b"payload").expect_err("symlinked home must fail"),
        CodexExecError::InvalidConfig
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn rejects_a_nonempty_codex_working_directory() {
    let (directory, script) = temp_script("nonempty-workspace", "cat >/dev/null");
    let options = config(&script);
    fs::write(
        options.working_directory.join("ambient.txt"),
        "must stay local",
    )
    .expect("seed ambient file");
    assert_eq!(
        run_codex_exec(&options, b"payload").expect_err("nonempty cwd must fail closed"),
        CodexExecError::InvalidConfig
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn rejects_an_unbounded_timeout_configuration() {
    let (directory, script) = temp_script("invalid-timeout", "cat >/dev/null");
    let mut options = config(&script);
    options.timeout = Duration::MAX;
    assert_eq!(
        run_codex_exec(&options, b"payload").expect_err("timeout must be bounded"),
        CodexExecError::InvalidConfig
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn rejects_a_nonzero_codex_exit_without_echoing_stderr() {
    let (directory, script) = temp_script(
        "failure",
        "cat >/dev/null; printf 'secret-path' >&2; exit 7",
    );
    let error = run_codex_exec(&config(&script), b"payload").expect_err("Codex must fail");
    assert_eq!(error, CodexExecError::NonZero { code: Some(7) });
    assert!(!error.to_string().contains("secret-path"));
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn terminates_a_timed_out_codex_process() {
    let (directory, script) = temp_script("timeout", "sleep 5");
    let mut options = config(&script);
    options.timeout = Duration::from_millis(50);
    let error = run_codex_exec(&options, b"payload").expect_err("Codex must time out");
    assert_eq!(error, CodexExecError::Timeout);
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn closes_descendant_pipes_after_the_direct_codex_process_exits() {
    let (directory, script) = temp_script("descendant", "cat >/dev/null; sleep 5 & exit 0");
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        sender
            .send(run_codex_exec(&config(&script), b"payload"))
            .expect("send runner result");
    });
    let result = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("descendant pipes must not defeat runner cleanup")
        .expect("parent exits successfully");
    assert!(result.stdout.is_empty());
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn bounds_mocked_codex_output_before_returning_it() {
    let (directory, script) = temp_script(
        "oversized",
        "cat >/dev/null; dd if=/dev/zero bs=1024 count=300 2>/dev/null",
    );
    let error = run_codex_exec(&config(&script), b"payload").expect_err("output must be bounded");
    assert_eq!(
        error,
        CodexExecError::OutputTooLarge {
            maximum: MAX_CODEX_OUTPUT_BYTES
        }
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}
