use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use key_insights::{analyze_jsonl, render_markdown, render_summary_json};
use serde::{Deserialize, Serialize};

mod publication;
mod recovery;
mod secure_fs;

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
    let input = arguments.get(1).map(PathBuf::from).ok_or_else(usage)?;
    let mut summary_path = None;
    let mut report_path = None;
    let mut index = 2;
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
            "--summary" | "--report" => return Err(format!("duplicate option {flag}")),
            _ => return Err(format!("unknown option {flag}")),
        }
        index += 2;
    }

    let summary_path = summary_path.ok_or_else(|| "missing --summary path".to_owned())?;
    let report_path = report_path.ok_or_else(|| "missing --report path".to_owned())?;
    let paths = resolve_paths(&input, &summary_path, &report_path)?;
    recover_outputs_anchored(&paths.summary, &paths.report)?;

    let input_file = File::open(&paths.input)
        .map_err(|error| format!("failed to open {}: {error}", paths.input.display()))?;
    let summary = analyze_jsonl(BufReader::new(input_file)).map_err(|error| error.to_string())?;
    let summary_output =
        StagedOutput::create(&paths.summary, render_summary_json(&summary).as_bytes())?;
    let report_output = StagedOutput::create(&paths.report, render_markdown(&summary).as_bytes())?;
    publish_pair(summary_output, report_output)?;
    Ok(())
}

struct ResolvedPaths {
    input: PathBuf,
    summary: ResolvedOutputPath,
    report: ResolvedOutputPath,
}

fn resolve_paths(input: &Path, summary: &Path, report: &Path) -> Result<ResolvedPaths, String> {
    let input = resolve_input_path(input)?;
    let summary = resolve_output_path(summary)?;
    let report = resolve_output_path(report)?;
    if same_file(&input, summary.as_path()) || same_file(&input, report.as_path()) {
        return Err("output paths must not overwrite the input log".to_owned());
    }
    if output_paths_may_collide(summary.as_path(), report.as_path())? {
        return Err("summary and report paths must be different".to_owned());
    }
    Ok(ResolvedPaths {
        input,
        summary,
        report,
    })
}

fn resolve_input_path(path: &Path) -> Result<PathBuf, String> {
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
    let metadata = fs::metadata(&resolved)
        .map_err(|error| format!("failed to inspect input {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("input must be a regular file: {}", path.display()));
    }
    Ok(resolved)
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
    "usage: key-insights analyze <input.jsonl> --summary <summary.json> --report <report.md>"
        .to_owned()
}

#[cfg(all(test, unix))]
mod tests;
