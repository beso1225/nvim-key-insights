use std::{
    collections::HashSet,
    env,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use key_insights::{
    MAX_SESSIONS_PER_LOG, analyze_jsonl_inputs, render_markdown, render_summary_json,
};
use serde::{Deserialize, Serialize};

mod discovery;
mod publication;
mod recovery;
mod secure_fs;

use discovery::*;
use publication::*;
use recovery::*;
use secure_fs::*;

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("key-insights: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Vec<std::ffi::OsString>) -> Result<(), String> {
    let command = arguments.first().and_then(|value| value.to_str());
    if command != Some("analyze") {
        return Err(usage());
    }
    let mut input_paths = Vec::new();
    let mut index = 1;
    while index < arguments.len() {
        let argument = &arguments[index];
        if argument == OsStr::new("--summary")
            || argument == OsStr::new("--report")
            || argument == OsStr::new("--session-dir")
            || argument == OsStr::new("--keymap-snapshot")
            || argument == OsStr::new("--keymap-snapshot-identity")
        {
            break;
        }
        if let Some(option) = argument.to_str().filter(|value| value.starts_with("--")) {
            return Err(format!("unknown option {option}"));
        }
        input_paths.push(PathBuf::from(argument));
        index += 1;
    }
    let mut session_directory = None;
    let mut summary_path = None;
    let mut report_path = None;
    let mut keymap_snapshot_path = None;
    let mut keymap_snapshot_identity = None;
    while index < arguments.len() {
        let flag = arguments[index]
            .to_str()
            .ok_or_else(|| "arguments must be valid UTF-8".to_owned())?;
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag {
            "--summary" if summary_path.is_none() => summary_path = Some(PathBuf::from(value)),
            "--report" if report_path.is_none() => report_path = Some(PathBuf::from(value)),
            "--session-dir" if session_directory.is_none() => {
                session_directory = Some(PathBuf::from(value))
            }
            "--keymap-snapshot" if keymap_snapshot_path.is_none() => {
                keymap_snapshot_path = Some(PathBuf::from(value))
            }
            "--keymap-snapshot-identity" if keymap_snapshot_identity.is_none() => {
                keymap_snapshot_identity = Some(
                    value
                        .to_str()
                        .ok_or_else(|| "snapshot identity must be valid UTF-8".to_owned())?
                        .to_owned(),
                )
            }
            "--summary"
            | "--report"
            | "--session-dir"
            | "--keymap-snapshot"
            | "--keymap-snapshot-identity" => {
                return Err(format!("duplicate option {flag}"));
            }
            _ => return Err(format!("unknown option {flag}")),
        }
        index += 2;
    }

    let summary_path = summary_path.ok_or_else(|| "missing --summary path".to_owned())?;
    let report_path = report_path.ok_or_else(|| "missing --report path".to_owned())?;
    let _keymap_snapshot = match (keymap_snapshot_path, keymap_snapshot_identity) {
        (Some(path), Some(identity)) => Some(open_snapshot_input(&path, &identity)?),
        (None, None) => None,
        _ => {
            return Err(
                "--keymap-snapshot and --keymap-snapshot-identity must be used together".to_owned(),
            );
        }
    };
    if !input_paths.is_empty() && session_directory.is_some() {
        return Err("explicit inputs and --session-dir are mutually exclusive".to_owned());
    }
    let inputs = match session_directory {
        Some(directory) => discover_session_inputs(&directory)?,
        None if !input_paths.is_empty() => {
            resolve_explicit_inputs(input_paths.iter().map(PathBuf::as_path))?
        }
        None => return Err(usage()),
    };
    let paths = resolve_paths_for_inputs(inputs, &summary_path, &report_path)?;
    recover_outputs_anchored(&paths.summary, &paths.report)?;

    let readers = paths.inputs.iter().map(|input| BufReader::new(&input.file));
    let summary = analyze_jsonl_inputs(readers).map_err(|error| match error.input_index {
        Some(index) => format!(
            "failed to analyze {}: {}",
            paths.inputs[index].path.display(),
            error.error
        ),
        None => error.error.to_string(),
    })?;
    let summary_output =
        StagedOutput::create(&paths.summary, render_summary_json(&summary).as_bytes())?;
    let report_output = StagedOutput::create(&paths.report, render_markdown(&summary).as_bytes())?;
    publish_pair(summary_output, report_output)?;
    Ok(())
}

#[cfg(unix)]
fn open_snapshot_input(path: &Path, expected_identity: &str) -> Result<File, String> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "failed to open keymap snapshot".to_owned())?;
    let metadata = file
        .metadata()
        .map_err(|_| "failed to inspect keymap snapshot".to_owned())?;
    let actual_identity = format!(
        "file:{}:{}:{}:{}:{}",
        metadata.dev(),
        metadata.ino(),
        metadata.size(),
        metadata.mtime(),
        metadata.mtime_nsec()
    );
    let private_mode = metadata.mode() & 0o7777 == 0o600;
    // SAFETY: geteuid has no preconditions and does not mutate memory.
    let owned = metadata.uid() == unsafe { libc::geteuid() };
    if !metadata.is_file()
        || metadata.nlink() != 1
        || !private_mode
        || !owned
        || actual_identity != expected_identity
    {
        return Err("keymap snapshot identity or permissions changed".to_owned());
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_snapshot_input(path: &Path, expected_identity: &str) -> Result<File, String> {
    let file = File::open(path).map_err(|_| "failed to open keymap snapshot".to_owned())?;
    let metadata = file
        .metadata()
        .map_err(|_| "failed to inspect keymap snapshot".to_owned())?;
    let actual_identity = format!("file:{}", metadata.len());
    if !metadata.is_file() || actual_identity != expected_identity {
        return Err("keymap snapshot identity changed".to_owned());
    }
    Ok(file)
}

struct ResolvedPaths {
    inputs: Vec<ResolvedInputPath>,
    summary: ResolvedOutputPath,
    report: ResolvedOutputPath,
}

#[derive(Debug)]
struct ResolvedInputPath {
    path: PathBuf,
    file: File,
    identity: InputIdentity,
}

#[cfg(unix)]
type InputIdentity = (u64, u64);

#[cfg(not(unix))]
type InputIdentity = PathBuf;

#[cfg(test)]
fn resolve_paths(input: &Path, summary: &Path, report: &Path) -> Result<ResolvedPaths, String> {
    resolve_input_paths(std::iter::once(input), summary, report)
}

#[cfg(test)]
fn resolve_input_paths<'a, I>(
    input_paths: I,
    summary: &Path,
    report: &Path,
) -> Result<ResolvedPaths, String>
where
    I: IntoIterator<Item = &'a Path>,
{
    let inputs = resolve_explicit_inputs(input_paths)?;
    resolve_paths_for_inputs(inputs, summary, report)
}

fn resolve_explicit_inputs<'a, I>(input_paths: I) -> Result<Vec<ResolvedInputPath>, String>
where
    I: IntoIterator<Item = &'a Path>,
{
    let mut identities = HashSet::new();
    let mut inputs = Vec::new();
    for input_path in input_paths {
        if inputs.len() >= MAX_SESSIONS_PER_LOG {
            return Err(format!(
                "input count exceeds the limit of {MAX_SESSIONS_PER_LOG}"
            ));
        }
        let input = resolve_input_path(input_path)?;
        if !identities.insert(owned_input_identity(&input.identity)) {
            return Err(format!("duplicate input: {}", input.path.display()));
        }
        inputs.push(input);
    }
    if inputs.is_empty() {
        return Err("at least one analysis input is required".to_owned());
    }
    Ok(inputs)
}

fn resolve_paths_for_inputs(
    inputs: Vec<ResolvedInputPath>,
    summary: &Path,
    report: &Path,
) -> Result<ResolvedPaths, String> {
    let summary = resolve_output_path(summary)?;
    let report = resolve_output_path(report)?;
    for input in &inputs {
        if output_matches_input(input, &summary)? || output_matches_input(input, &report)? {
            return Err("output paths must not overwrite the input log".to_owned());
        }
    }
    if output_paths_may_collide(summary.as_path(), report.as_path())? {
        return Err("summary and report paths must be different".to_owned());
    }
    reject_recovery_artifact_collisions(summary.as_path(), report.as_path())?;
    Ok(ResolvedPaths {
        inputs,
        summary,
        report,
    })
}

fn resolve_input_path(path: &Path) -> Result<ResolvedInputPath, String> {
    let resolved = fs::canonicalize(path)
        .map_err(|error| format!("failed to resolve input {}: {error}", path.display()))?;
    if resolved
        .file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with(".jsonl.part"))
    {
        return Err(format!(
            "input is an incomplete collector artifact: {}",
            path.display()
        ));
    }
    let (file, metadata) = open_input_file(&resolved)?;
    let identity = input_identity(&resolved, &metadata);
    Ok(ResolvedInputPath {
        path: resolved,
        file,
        identity,
    })
}

#[cfg(unix)]
fn open_input_file(path: &Path) -> Result<(File, fs::Metadata), String> {
    use std::os::unix::fs::OpenOptionsExt;

    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| format!("failed to open input {}: {error}", path.display()))?;
    inspect_input_file(path, file)
}

#[cfg(not(unix))]
fn open_input_file(path: &Path) -> Result<(File, fs::Metadata), String> {
    let file = File::open(path)
        .map_err(|error| format!("failed to open input {}: {error}", path.display()))?;
    inspect_input_file(path, file)
}

fn inspect_input_file(path: &Path, file: File) -> Result<(File, fs::Metadata), String> {
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect input {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("input must be a regular file: {}", path.display()));
    }
    Ok((file, metadata))
}

#[cfg(unix)]
fn input_identity(_path: &Path, metadata: &fs::Metadata) -> InputIdentity {
    use std::os::unix::fs::MetadataExt;

    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn input_identity(path: &Path, _metadata: &fs::Metadata) -> InputIdentity {
    path.to_owned()
}

#[cfg(unix)]
fn owned_input_identity(identity: &InputIdentity) -> InputIdentity {
    *identity
}

#[cfg(not(unix))]
fn owned_input_identity(identity: &InputIdentity) -> InputIdentity {
    identity.clone()
}

#[cfg(unix)]
fn output_matches_input(
    input: &ResolvedInputPath,
    output: &ResolvedOutputPath,
) -> Result<bool, String> {
    let name = output
        .path
        .file_name()
        .ok_or_else(|| format!("output path has no file name: {}", output.path.display()))?;
    let metadata = output.directory.child_metadata(name).map_err(|error| {
        format!(
            "failed to inspect output {}: {error}",
            output.path.display()
        )
    })?;
    Ok(metadata
        .is_some_and(|metadata| (metadata.device_u64(), metadata.inode_u64()) == input.identity))
}

#[cfg(not(unix))]
fn output_matches_input(
    input: &ResolvedInputPath,
    output: &ResolvedOutputPath,
) -> Result<bool, String> {
    Ok(input.path == output.path || same_file(&input.path, &output.path))
}

struct ResolvedOutputPath {
    path: PathBuf,
    directory: Arc<ResolvedDirectory>,
}

impl ResolvedOutputPath {
    fn as_path(&self) -> &Path {
        &self.path
    }
}

impl std::ops::Deref for ResolvedOutputPath {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.as_path()
    }
}

fn resolve_output_path(path: &Path) -> Result<ResolvedOutputPath, String> {
    let resolved = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
            "output path must not be a symlink: {}",
            path.display()
        )),
        Ok(metadata) if metadata.is_dir() => Err(format!(
            "output path must not be a directory: {}",
            path.display()
        )),
        Ok(metadata) if !metadata.is_file() => Err(format!(
            "output path must be a regular file: {}",
            path.display()
        )),
        Ok(_) => fs::canonicalize(path)
            .map_err(|error| format!("failed to resolve output {}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            let name = path
                .file_name()
                .ok_or_else(|| format!("output path has no file name: {}", path.display()))?;
            let parent = fs::canonicalize(parent).map_err(|error| {
                format!("failed to resolve parent {}: {error}", parent.display())
            })?;
            Ok(parent.join(name))
        }
        Err(error) => Err(format!(
            "failed to inspect output {}: {error}",
            path.display()
        )),
    }?;
    let parent = resolved
        .parent()
        .ok_or_else(|| format!("output path has no parent: {}", resolved.display()))?;
    let directory = ResolvedDirectory::open(parent)?;
    Ok(ResolvedOutputPath {
        path: resolved,
        directory: Arc::new(directory),
    })
}

fn usage() -> String {
    "usage: key-insights analyze (<input.jsonl>... | --session-dir <directory>) --summary <summary.json> --report <report.md> [--keymap-snapshot <snapshot.json> --keymap-snapshot-identity <identity>]".to_owned()
}

#[cfg(all(test, unix))]
mod tests;
