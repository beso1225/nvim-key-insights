use std::{ffi::OsString, path::PathBuf, time::Duration};
#[cfg(unix)]
use std::{
    io::{self, Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Instant,
};

use crate::MAX_CODEX_PAYLOAD_BYTES;

/// Maximum output retained from a Codex process.
pub const MAX_CODEX_OUTPUT_BYTES: usize = MAX_CODEX_PAYLOAD_BYTES;
/// Maximum execution time accepted from callers.
pub const MAX_CODEX_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct CodexExecConfig {
    pub binary: PathBuf,
    pub codex_home: PathBuf,
    pub output_schema: PathBuf,
    pub path_environment: OsString,
    pub working_directory: PathBuf,
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexExecError {
    UnsupportedPlatform,
    InvalidConfig,
    Spawn,
    Io,
    Timeout,
    InputTooLarge { maximum: usize },
    OutputTooLarge { maximum: usize },
    NonZero { code: Option<i32> },
}

impl std::fmt::Display for CodexExecError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                formatter.write_str("Codex execution requires Unix process-group isolation")
            }
            Self::InvalidConfig => formatter.write_str("invalid Codex runner configuration"),
            Self::Spawn => formatter.write_str("failed to start Codex"),
            Self::Io => formatter.write_str("Codex process I/O failed"),
            Self::Timeout => formatter.write_str("Codex execution timed out"),
            Self::InputTooLarge { maximum } => {
                write!(formatter, "Codex input exceeds the {maximum}-byte limit")
            }
            Self::OutputTooLarge { maximum } => {
                write!(formatter, "Codex output exceeds the {maximum}-byte limit")
            }
            Self::NonZero { code } => write!(formatter, "Codex exited unsuccessfully ({code:?})"),
        }
    }
}

impl std::error::Error for CodexExecError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexExecResult {
    pub stdout: Vec<u8>,
}

/// Build argv without invoking a shell or interpolating the payload.
pub fn build_codex_exec_argv(config: &CodexExecConfig) -> Vec<OsString> {
    vec![
        OsString::from("exec"),
        OsString::from("--ephemeral"),
        OsString::from("--ignore-user-config"),
        OsString::from("--ignore-rules"),
        OsString::from("--strict-config"),
        OsString::from("--skip-git-repo-check"),
        OsString::from("--cd"),
        config.working_directory.as_os_str().to_owned(),
        OsString::from("--config"),
        OsString::from(r#"shell_environment_policy.inherit="none""#),
        OsString::from("--config"),
        OsString::from(r#"approval_policy="never""#),
        OsString::from("--config"),
        OsString::from(r#"default_permissions="key-insights-payload-only""#),
        OsString::from("--config"),
        OsString::from(
            r#"permissions.key-insights-payload-only.filesystem={":root"="deny",":minimal"="read"}"#,
        ),
        OsString::from("--config"),
        OsString::from(r#"permissions.key-insights-payload-only.network.enabled=false"#),
        OsString::from("--output-schema"),
        config.output_schema.as_os_str().to_owned(),
    ]
}

/// Run `codex exec` with a sanitized payload on stdin.
///
/// Authentication is inherited from the user's saved Codex session. This
/// function deliberately does not inspect or create API-key environment
/// variables and never invokes a shell.
#[cfg(not(unix))]
pub fn run_codex_exec(
    _config: &CodexExecConfig,
    _payload: &[u8],
) -> Result<CodexExecResult, CodexExecError> {
    Err(CodexExecError::UnsupportedPlatform)
}

#[cfg(unix)]
pub fn run_codex_exec(
    config: &CodexExecConfig,
    payload: &[u8],
) -> Result<CodexExecResult, CodexExecError> {
    if config.timeout.is_zero()
        || config.timeout > MAX_CODEX_TIMEOUT
        || config.max_output_bytes == 0
        || config.max_output_bytes > MAX_CODEX_OUTPUT_BYTES
        || !config.binary.is_absolute()
        || !valid_codex_home(&config.codex_home)
        || config.path_environment.is_empty()
        || !valid_working_directory(&config.working_directory)
    {
        return Err(CodexExecError::InvalidConfig);
    }
    if payload.len() > MAX_CODEX_PAYLOAD_BYTES {
        return Err(CodexExecError::InputTooLarge {
            maximum: MAX_CODEX_PAYLOAD_BYTES,
        });
    }
    let mut command = Command::new(&config.binary);
    command
        .args(build_codex_exec_argv(config))
        .current_dir(&config.working_directory)
        .env_clear()
        .env("CODEX_HOME", &config.codex_home)
        .env("PATH", &config.path_environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_process_group(&mut command);
    let mut child = command.spawn().map_err(|_| CodexExecError::Spawn)?;
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate_child(&mut child);
            let _ = child.wait();
            return Err(CodexExecError::Io);
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            terminate_child(&mut child);
            let _ = child.wait();
            return Err(CodexExecError::Io);
        }
    };
    let stdout_overflowed = Arc::new(AtomicBool::new(false));
    let stderr_overflowed = Arc::new(AtomicBool::new(false));
    let stdout_thread = spawn_reader(stdout, config.max_output_bytes, stdout_overflowed.clone());
    let stderr_thread = spawn_reader(stderr, config.max_output_bytes, stderr_overflowed.clone());

    let stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            terminate_child(&mut child);
            let _ = child.wait();
            return Err(CodexExecError::Io);
        }
    };
    let payload = payload.to_owned();
    let stdin_thread = thread::spawn(move || {
        let mut stdin = stdin;
        let result = stdin.write_all(&payload);
        drop(stdin);
        result
    });

    let deadline = Instant::now()
        .checked_add(config.timeout)
        .ok_or(CodexExecError::InvalidConfig)?;
    let mut timed_out = false;
    let status = loop {
        if stdout_overflowed.load(Ordering::Acquire) || stderr_overflowed.load(Ordering::Acquire) {
            terminate_child(&mut child);
            break child.wait().map_err(|_| CodexExecError::Io)?;
        }
        if let Some(status) = child.try_wait().map_err(|_| CodexExecError::Io)? {
            // The direct process may have exited while a descendant still
            // holds one of the inherited pipes open. The dedicated process
            // group keeps reader joins bounded and prevents leaked helpers.
            terminate_child(&mut child);
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            terminate_child(&mut child);
            break child.wait().map_err(|_| CodexExecError::Io)?;
        }
        thread::sleep(Duration::from_millis(2));
    };
    let stdout = stdout_thread
        .join()
        .map_err(|_| CodexExecError::Io)?
        .map_err(|_| CodexExecError::Io)?;
    let _stderr = stderr_thread
        .join()
        .map_err(|_| CodexExecError::Io)?
        .map_err(|_| CodexExecError::Io)?;
    let stdin_result = stdin_thread.join().map_err(|_| CodexExecError::Io)?;

    if timed_out {
        return Err(CodexExecError::Timeout);
    }
    if stdin_result.is_err() {
        return Err(CodexExecError::Io);
    }
    if stdout_overflowed.load(Ordering::Acquire) || stderr_overflowed.load(Ordering::Acquire) {
        return Err(CodexExecError::OutputTooLarge {
            maximum: config.max_output_bytes,
        });
    }
    if !status.success() {
        return Err(CodexExecError::NonZero {
            code: status.code(),
        });
    }
    Ok(CodexExecResult { stdout })
}

#[cfg(unix)]
fn valid_codex_home(path: &Path) -> bool {
    path.is_absolute()
        && path
            .symlink_metadata()
            .map(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
            .unwrap_or(false)
}

#[cfg(unix)]
fn spawn_reader<R>(
    mut reader: R,
    limit: usize,
    overflowed: Arc<AtomicBool>,
) -> thread::JoinHandle<io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0_u8; 8192];
        let mut total = 0_usize;
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    total = total.saturating_add(read);
                    if total <= limit {
                        output.extend_from_slice(&buffer[..read]);
                    } else {
                        overflowed.store(true, Ordering::Release);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Ok(output)
    })
}

#[cfg(unix)]
fn valid_working_directory(path: &std::path::Path) -> bool {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    // SAFETY: geteuid has no arguments or memory-safety preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    if !path.is_absolute()
        || !metadata.file_type().is_dir()
        || metadata.uid() != effective_uid
        || metadata.permissions().mode() & 0o077 != 0
    {
        return false;
    }
    std::fs::read_dir(path)
        .ok()
        .and_then(|mut entries| entries.next().transpose().ok())
        .is_some_and(|entry| entry.is_none())
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // SAFETY: setpgid only changes the child process group before exec and does
    // not access Rust-managed memory.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(unix)]
fn terminate_child(child: &mut std::process::Child) {
    let process_group = -(child.id() as libc::pid_t);
    // SAFETY: the PID comes from the child just spawned by this process; a
    // negative PID targets only its dedicated process group.
    unsafe {
        libc::kill(process_group, libc::SIGKILL);
    }
}
