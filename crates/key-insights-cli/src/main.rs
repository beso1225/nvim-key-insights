use std::{
    env,
    fs::{self, File},
    io::BufReader,
    path::{Path, PathBuf},
    process::ExitCode,
};

use key_insights::{analyze_jsonl, render_markdown, render_summary_json};

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
    reject_overlapping_paths(&input, &summary_path, &report_path)?;

    let input_file = File::open(&input)
        .map_err(|error| format!("failed to open {}: {error}", input.display()))?;
    let summary = analyze_jsonl(BufReader::new(input_file)).map_err(|error| error.to_string())?;
    fs::write(&summary_path, render_summary_json(&summary))
        .map_err(|error| format!("failed to write {}: {error}", summary_path.display()))?;
    fs::write(&report_path, render_markdown(&summary))
        .map_err(|error| format!("failed to write {}: {error}", report_path.display()))?;
    Ok(())
}

fn reject_overlapping_paths(input: &Path, summary: &Path, report: &Path) -> Result<(), String> {
    let input = resolve_path(input)?;
    let summary = resolve_path(summary)?;
    let report = resolve_path(report)?;
    if same_file(&input, &summary) || same_file(&input, &report) {
        return Err("output paths must not overwrite the input log".to_owned());
    }
    if same_file(&summary, &report) {
        return Err("summary and report paths must be different".to_owned());
    }
    Ok(())
}

fn resolve_path(path: &Path) -> Result<PathBuf, String> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().unwrap_or_else(|| Path::new("."));
            let name = path
                .file_name()
                .ok_or_else(|| format!("output path has no file name: {}", path.display()))?;
            let parent = fs::canonicalize(parent).map_err(|error| {
                format!("failed to resolve parent {}: {error}", parent.display())
            })?;
            Ok(parent.join(name))
        }
        Err(error) => Err(format!("failed to resolve {}: {error}", path.display())),
    }
}

fn same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    same_file_metadata(left, right)
}

#[cfg(unix)]
fn same_file_metadata(left: &Path, right: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    match (fs::metadata(left), fs::metadata(right)) {
        (Ok(left), Ok(right)) => left.dev() == right.dev() && left.ino() == right.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_file_metadata(_left: &Path, _right: &Path) -> bool {
    false
}

fn usage() -> String {
    "usage: key-insights analyze <input.jsonl> --summary <summary.json> --report <report.md>"
        .to_owned()
}
