use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{BufReader, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::atomic::{AtomicU64, Ordering},
};

use key_insights::{analyze_jsonl, render_markdown, render_summary_json};

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
    summary: PathBuf,
    report: PathBuf,
}

fn resolve_paths(input: &Path, summary: &Path, report: &Path) -> Result<ResolvedPaths, String> {
    let input = resolve_input_path(input)?;
    let summary = resolve_output_path(summary)?;
    let report = resolve_output_path(report)?;
    if same_file(&input, &summary) || same_file(&input, &report) {
        return Err("output paths must not overwrite the input log".to_owned());
    }
    if output_paths_may_collide(&summary, &report)? {
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

fn resolve_output_path(path: &Path) -> Result<PathBuf, String> {
    match fs::symlink_metadata(path) {
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
    }
}

fn same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    same_file_metadata(left, right)
}

fn output_paths_may_collide(left: &Path, right: &Path) -> Result<bool, String> {
    if same_file(left, right) {
        return Ok(true);
    }
    let (Some(left_parent), Some(right_parent)) = (left.parent(), right.parent()) else {
        return Ok(false);
    };
    if !same_file(left_parent, right_parent) {
        return Ok(false);
    }
    let left_name = left
        .file_name()
        .ok_or_else(|| format!("output path has no file name: {}", left.display()))?;
    let right_name = right
        .file_name()
        .ok_or_else(|| format!("output path has no file name: {}", right.display()))?;
    probe_name_collision(left_parent, left_name, right_name)
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

struct NameProbeDirectory {
    path: PathBuf,
    files: Vec<PathBuf>,
}

impl NameProbeDirectory {
    fn create(parent: &Path) -> Result<Self, String> {
        for _attempt in 0..100 {
            let identifier = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(
                ".key-insights.name-probe-{}-{identifier}",
                std::process::id()
            ));
            match create_private_directory(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        files: Vec::new(),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!(
                        "failed to create output-name probe in {}: {error}",
                        parent.display()
                    ));
                }
            }
        }
        Err(format!(
            "failed to reserve output-name probe in {}",
            parent.display()
        ))
    }
}

impl Drop for NameProbeDirectory {
    fn drop(&mut self) {
        for path in self.files.iter().rev() {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir(&self.path);
    }
}

fn probe_name_collision(
    parent: &Path,
    left_name: &std::ffi::OsStr,
    right_name: &std::ffi::OsStr,
) -> Result<bool, String> {
    let mut probe = NameProbeDirectory::create(parent)?;
    let left = probe.path.join(left_name);
    let left_file = open_private_new_file(&left).map_err(|error| {
        format!(
            "failed to probe output name {}: {error}",
            left_name.to_string_lossy()
        )
    })?;
    drop(left_file);
    probe.files.push(left);

    let right = probe.path.join(right_name);
    match open_private_new_file(&right) {
        Ok(file) => {
            drop(file);
            probe.files.push(right);
            Ok(false)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(true),
        Err(error) => Err(format!(
            "failed to probe output name {}: {error}",
            right_name.to_string_lossy()
        )),
    }
}

struct StagedOutput {
    temporary_path: Option<PathBuf>,
    destination: PathBuf,
}

impl StagedOutput {
    fn create(destination: &Path, contents: &[u8]) -> Result<Self, String> {
        let parent = destination
            .parent()
            .ok_or_else(|| format!("output path has no parent: {}", destination.display()))?;
        let name = destination
            .file_name()
            .ok_or_else(|| format!("output path has no file name: {}", destination.display()))?
            .to_string_lossy();

        for _attempt in 0..100 {
            let identifier = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let temporary_path = parent.join(format!(
                ".{name}.key-insights.tmp-{}-{identifier}",
                std::process::id()
            ));
            match open_private_new_file(&temporary_path) {
                Ok(mut file) => {
                    let staged = Self {
                        temporary_path: Some(temporary_path),
                        destination: destination.to_owned(),
                    };
                    let write_result = file.write_all(contents).and_then(|()| file.sync_all());
                    drop(file);
                    write_result.map_err(|error| {
                        format!(
                            "failed to stage and sync {}: {error}",
                            staged.destination.display()
                        )
                    })?;
                    return Ok(staged);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!(
                        "failed to create staged output for {}: {error}",
                        destination.display()
                    ));
                }
            }
        }
        Err(format!(
            "failed to reserve a staged output for {}",
            destination.display()
        ))
    }

    fn publish(mut self) -> Result<(), String> {
        let temporary_path = self
            .temporary_path
            .take()
            .expect("staged output has a temporary path");
        if let Err(error) = fs::rename(&temporary_path, &self.destination) {
            self.temporary_path = Some(temporary_path);
            return Err(format!(
                "failed to publish {}: {error}",
                self.destination.display()
            ));
        }
        Ok(())
    }
}

impl Drop for StagedOutput {
    fn drop(&mut self) {
        if let Some(path) = &self.temporary_path {
            let _ = fs::remove_file(path);
        }
    }
}

struct OutputBackup {
    destination: PathBuf,
    backup_path: Option<PathBuf>,
}

impl OutputBackup {
    fn capture(destination: &Path) -> Result<Self, String> {
        match fs::symlink_metadata(destination) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                destination: destination.to_owned(),
                backup_path: None,
            }),
            Ok(metadata) if metadata.is_dir() => Err(format!(
                "output destination became a directory: {}",
                destination.display()
            )),
            Ok(metadata) if !metadata.is_file() => Err(format!(
                "output destination became a non-regular file: {}",
                destination.display()
            )),
            Ok(_) => {
                let backup_path = move_to_unused_sibling(destination, "backup")?;
                Ok(Self {
                    destination: destination.to_owned(),
                    backup_path: Some(backup_path),
                })
            }
            Err(error) => Err(format!(
                "failed to inspect output {} before publication: {error}",
                destination.display()
            )),
        }
    }

    fn restore(&mut self) -> Result<(), String> {
        match fs::symlink_metadata(&self.destination) {
            Ok(metadata) if metadata.is_dir() => {
                return Err(format!(
                    "cannot restore output over directory {}",
                    self.destination.display()
                ));
            }
            Ok(_) => fs::remove_file(&self.destination).map_err(|error| {
                format!(
                    "failed to remove unpublished output {}: {error}",
                    self.destination.display()
                )
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "failed to inspect output {} during rollback: {error}",
                    self.destination.display()
                ));
            }
        }
        if let Some(backup_path) = &self.backup_path {
            fs::rename(backup_path, &self.destination).map_err(|error| {
                format!(
                    "failed to restore previous output {}: {error}",
                    self.destination.display()
                )
            })?;
            self.backup_path = None;
        }
        Ok(())
    }

    fn discard(mut self) {
        if let Some(backup_path) = self.backup_path.take() {
            let _ = fs::remove_file(backup_path);
        }
    }
}

fn publish_pair(summary: StagedOutput, report: StagedOutput) -> Result<(), String> {
    let mut summary_backup = OutputBackup::capture(&summary.destination)?;
    let mut report_backup = match OutputBackup::capture(&report.destination) {
        Ok(backup) => backup,
        Err(error) => {
            return Err(with_rollback(error, [&mut summary_backup]));
        }
    };

    if let Err(error) = summary.publish() {
        return Err(with_rollback(
            error,
            [&mut summary_backup, &mut report_backup],
        ));
    }
    if let Err(error) = report.publish() {
        return Err(with_rollback(
            error,
            [&mut summary_backup, &mut report_backup],
        ));
    }

    summary_backup.discard();
    report_backup.discard();
    Ok(())
}

fn with_rollback<const N: usize>(error: String, backups: [&mut OutputBackup; N]) -> String {
    let rollback_errors: Vec<_> = backups
        .into_iter()
        .filter_map(|backup| backup.restore().err())
        .collect();
    if rollback_errors.is_empty() {
        error
    } else {
        format!("{error}; rollback failed: {}", rollback_errors.join("; "))
    }
}

fn move_to_unused_sibling(destination: &Path, label: &str) -> Result<PathBuf, String> {
    let parent = destination
        .parent()
        .ok_or_else(|| format!("output path has no parent: {}", destination.display()))?;
    let name = destination
        .file_name()
        .ok_or_else(|| format!("output path has no file name: {}", destination.display()))?
        .to_string_lossy();
    for _attempt in 0..100 {
        let identifier = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{name}.key-insights.{label}-{}-{identifier}",
            std::process::id()
        ));
        match rename_without_replacement(destination, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "failed to preserve existing output {}: {error}",
                    destination.display()
                ));
            }
        }
    }
    Err(format!(
        "failed to reserve a backup path for {}",
        destination.display()
    ))
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn rename_without_replacement(source: &Path, destination: &Path) -> std::io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    renameat_with(CWD, source, CWD, destination, RenameFlags::NOREPLACE).map_err(Into::into)
}

#[cfg(windows)]
fn rename_without_replacement(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple", windows)))]
fn rename_without_replacement(_source: &Path, _destination: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic no-replace rename is unavailable on this platform",
    ))
}

fn open_private_new_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn create_private_directory(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn usage() -> String {
    "usage: key-insights analyze <input.jsonl> --summary <summary.json> --report <report.md>"
        .to_owned()
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        fs,
        os::unix::fs::symlink,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        OutputBackup, StagedOutput, publish_pair, rename_without_replacement, resolve_paths,
    };

    #[test]
    fn atomic_publication_replaces_a_swapped_symlink_without_following_it() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-publication-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create test directory");
        let input = directory.join("input.jsonl");
        let summary = directory.join("summary.json");
        let report = directory.join("report.md");
        fs::write(&input, "raw input\n").expect("write input");
        let paths = resolve_paths(&input, &summary, &report).expect("resolve safe paths");

        symlink(&input, &summary).expect("swap output to input symlink");
        StagedOutput::create(&paths.summary, b"summary\n")
            .expect("stage output")
            .publish()
            .expect("publish output");

        assert_eq!(
            fs::read_to_string(&input).expect("input survives"),
            "raw input\n"
        );
        assert_eq!(
            fs::read_to_string(&summary).expect("read output"),
            "summary\n"
        );
        assert!(
            !fs::symlink_metadata(&summary)
                .expect("output metadata")
                .file_type()
                .is_symlink()
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn failed_second_publication_restores_the_previous_pair() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-rollback-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create test directory");
        let summary = directory.join("summary.json");
        let report = directory.join("report.md");
        fs::write(&summary, "old summary\n").expect("write old summary");
        fs::write(&report, "old report\n").expect("write old report");
        let summary_output =
            StagedOutput::create(&summary, b"new summary\n").expect("stage summary");
        let report_output = StagedOutput::create(&report, b"new report\n").expect("stage report");
        fs::remove_file(
            report_output
                .temporary_path
                .as_ref()
                .expect("report temporary path"),
        )
        .expect("force second publication failure");

        publish_pair(summary_output, report_output).expect_err("publication must fail");

        assert_eq!(
            fs::read_to_string(&summary).expect("read summary"),
            "old summary\n"
        );
        assert_eq!(
            fs::read_to_string(&report).expect("read report"),
            "old report\n"
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn backup_rename_never_replaces_an_existing_entry() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-backup-reservation-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create test directory");
        let source = directory.join("summary.json");
        let occupied_backup = directory.join("occupied-backup");
        fs::write(&source, "previous summary\n").expect("write prior output");
        fs::write(&occupied_backup, "unrelated\n").expect("write unrelated file");

        let error = rename_without_replacement(&source, &occupied_backup)
            .expect_err("occupied backup path must not be replaced");

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read_to_string(&source).expect("source survives"),
            "previous summary\n"
        );
        assert_eq!(
            fs::read_to_string(&occupied_backup).expect("unrelated backup survives"),
            "unrelated\n"
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn backup_capture_refuses_a_swapped_output_symlink() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-backup-symlink-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create test directory");
        let destination = directory.join("summary.json");
        let target = directory.join("target");
        fs::write(&target, "unrelated\n").expect("write symlink target");
        symlink(&target, &destination).expect("swap destination to symlink");

        let error = match OutputBackup::capture(&destination) {
            Ok(_) => panic!("symlink must be rejected"),
            Err(error) => error,
        };

        assert!(error.contains("non-regular file"));
        assert!(
            fs::symlink_metadata(&destination)
                .expect("symlink survives")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_to_string(&target).expect("read target"),
            "unrelated\n"
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }
}
