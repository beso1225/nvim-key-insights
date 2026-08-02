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
            .ok_or_else(|| format!("output path has no file name: {}", destination.display()))?;
        let label = bounded_file_label(name);

        for _attempt in 0..100 {
            let identifier = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let temporary_path = parent.join(format!(
                ".key-insights.{label}.tmp-{}-{identifier}",
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
                let backup_path = link_to_unused_sibling(destination, "backup")?;
                if let Err(error) = sync_parent_directory(destination) {
                    let _ = fs::remove_file(&backup_path);
                    return Err(format!(
                        "failed to sync backup for {}: {error}",
                        destination.display()
                    ));
                }
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
        if let Some(backup_path) = &self.backup_path {
            match fs::symlink_metadata(&self.destination) {
                Ok(metadata) if !metadata.is_file() => {
                    return Err(format!(
                        "cannot restore output over non-regular file {}",
                        self.destination.display()
                    ));
                }
                Ok(_) if same_file(backup_path, &self.destination) => {
                    fs::remove_file(backup_path).map_err(|error| {
                        format!(
                            "failed to remove redundant backup for {}: {error}",
                            self.destination.display()
                        )
                    })?;
                    self.backup_path = None;
                    return Ok(());
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "failed to inspect output {} during rollback: {error}",
                        self.destination.display()
                    ));
                }
            }
            fs::rename(backup_path, &self.destination).map_err(|error| {
                format!(
                    "failed to restore previous output {}: {error}",
                    self.destination.display()
                )
            })?;
            self.backup_path = None;
            sync_parent_directory(&self.destination).map_err(|error| {
                format!(
                    "failed to sync restored output {}: {error}",
                    self.destination.display()
                )
            })?;
            return Ok(());
        }

        match fs::symlink_metadata(&self.destination) {
            Ok(metadata) if !metadata.is_file() => {
                return Err(format!(
                    "cannot remove non-regular output during rollback: {}",
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
        sync_parent_directory(&self.destination).map_err(|error| {
            format!(
                "failed to sync removed output {}: {error}",
                self.destination.display()
            )
        })?;
        Ok(())
    }

    fn discard(mut self) {
        if let Some(backup_path) = self.backup_path.take() {
            let _ = fs::remove_file(backup_path);
        }
    }
}

fn publish_pair(summary: StagedOutput, report: StagedOutput) -> Result<(), String> {
    publish_pair_with_hook(summary, report, || {})
}

fn publish_pair_with_hook<F>(
    summary: StagedOutput,
    report: StagedOutput,
    after_summary: F,
) -> Result<(), String>
where
    F: FnOnce(),
{
    let _locks = OutputLocks::acquire(&summary.destination, &report.destination)?;
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
    after_summary();
    if let Err(error) = report.publish() {
        return Err(with_rollback(
            error,
            [&mut summary_backup, &mut report_backup],
        ));
    }
    if let Err(error) =
        sync_output_directories(&summary_backup.destination, &report_backup.destination)
    {
        return Err(with_rollback(
            error,
            [&mut summary_backup, &mut report_backup],
        ));
    }

    summary_backup.discard();
    report_backup.discard();
    Ok(())
}

fn sync_output_directories(summary: &Path, report: &Path) -> Result<(), String> {
    let summary_parent = summary
        .parent()
        .ok_or_else(|| format!("output path has no parent: {}", summary.display()))?;
    let report_parent = report
        .parent()
        .ok_or_else(|| format!("output path has no parent: {}", report.display()))?;
    sync_parent_directory(summary).map_err(|error| {
        format!(
            "failed to sync output directory {}: {error}",
            summary_parent.display()
        )
    })?;
    if !same_file(summary_parent, report_parent) {
        sync_parent_directory(report).map_err(|error| {
            format!(
                "failed to sync output directory {}: {error}",
                report_parent.display()
            )
        })?;
    }
    Ok(())
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing parent"))?;
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> std::io::Result<()> {
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

fn link_to_unused_sibling(destination: &Path, label: &str) -> Result<PathBuf, String> {
    let parent = destination
        .parent()
        .ok_or_else(|| format!("output path has no parent: {}", destination.display()))?;
    let name = destination
        .file_name()
        .ok_or_else(|| format!("output path has no file name: {}", destination.display()))?;
    let name = bounded_file_label(name);
    for _attempt in 0..100 {
        let identifier = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".key-insights.{name}.{label}-{}-{identifier}",
            std::process::id()
        ));
        match link_without_replacement(destination, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "failed to link existing output {} into a backup: {error}",
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

fn link_without_replacement(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::hard_link(source, destination)
}

struct OutputLocks {
    _files: Vec<File>,
}

impl OutputLocks {
    fn acquire(summary: &Path, report: &Path) -> Result<Self, String> {
        let mut lock_paths = vec![output_lock_path(summary)?, output_lock_path(report)?];
        lock_paths.sort();
        lock_paths.dedup();
        if lock_paths.len() == 2 && output_paths_may_collide(&lock_paths[0], &lock_paths[1])? {
            lock_paths.pop();
        }

        for lock_path in &lock_paths {
            if output_paths_may_collide(lock_path, summary)?
                || output_paths_may_collide(lock_path, report)?
            {
                return Err(format!(
                    "output path collides with publication lock: {}",
                    lock_path.display()
                ));
            }
        }

        let mut files = Vec::with_capacity(lock_paths.len());
        for lock_path in lock_paths {
            let file = open_private_lock_file(&lock_path).map_err(|error| {
                format!(
                    "failed to open publication lock {}: {error}",
                    lock_path.display()
                )
            })?;
            file.lock().map_err(|error| {
                format!(
                    "failed to acquire publication lock {}: {error}",
                    lock_path.display()
                )
            })?;
            if !open_file_matches_path(&file, &lock_path).map_err(|error| {
                format!(
                    "failed to verify publication lock {}: {error}",
                    lock_path.display()
                )
            })? {
                return Err(format!(
                    "publication lock path changed while acquiring it: {}",
                    lock_path.display()
                ));
            }
            files.push(file);
        }
        Ok(Self { _files: files })
    }
}

fn output_lock_path(destination: &Path) -> Result<PathBuf, String> {
    let parent = destination
        .parent()
        .ok_or_else(|| format!("output path has no parent: {}", destination.display()))?;
    let name = destination
        .file_name()
        .ok_or_else(|| format!("output path has no file name: {}", destination.display()))?;
    let label = bounded_file_label(name);
    Ok(parent.join(format!(".key-insights.lock-{label}")))
}

fn bounded_file_label(name: &std::ffi::OsStr) -> String {
    let mut label = String::new();
    for character in name.to_string_lossy().chars() {
        if label.len() + character.len_utf8() > 96 {
            break;
        }
        label.push(character);
    }
    if label.is_empty() {
        label.push_str("output");
    }
    label
}

fn open_private_lock_file(path: &Path) -> std::io::Result<File> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "lock path is not a regular file",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

#[cfg(unix)]
fn open_file_matches_path(file: &File, path: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let open = file.metadata()?;
    let path = fs::metadata(path)?;
    Ok(open.dev() == path.dev()
        && open.ino() == path.ino()
        && open.nlink() == 1
        && open.mode() & 0o077 == 0)
}

#[cfg(not(unix))]
fn open_file_matches_path(_file: &File, path: &Path) -> std::io::Result<bool> {
    Ok(fs::metadata(path)?.is_file())
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
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        OutputBackup, OutputLocks, StagedOutput, link_without_replacement, open_private_lock_file,
        output_lock_path, publish_pair, publish_pair_with_hook, resolve_paths,
    };

    #[test]
    fn staged_output_rename_does_not_follow_a_swapped_symlink() {
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
    fn backup_link_never_replaces_an_existing_entry() {
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

        let error = link_without_replacement(&source, &occupied_backup)
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
    fn backup_capture_keeps_the_previous_output_at_its_public_path() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-linked-backup-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create test directory");
        let destination = directory.join("summary.json");
        fs::write(&destination, "previous summary\n").expect("write prior output");

        let backup = OutputBackup::capture(&destination).expect("capture output");

        assert_eq!(
            fs::read_to_string(&destination).expect("public output remains available"),
            "previous summary\n"
        );
        backup.discard();
        assert_eq!(
            fs::read_to_string(&destination).expect("public output survives cleanup"),
            "previous summary\n"
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

    #[test]
    fn paired_publication_is_serialized_across_threads() {
        use std::{sync::mpsc, thread, time::Duration};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-pair-lock-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create test directory");
        let summary = directory.join("summary.json");
        let report = directory.join("report.md");
        let a_summary = StagedOutput::create(&summary, b"A summary\n").expect("stage A summary");
        let a_report = StagedOutput::create(&report, b"A report\n").expect("stage A report");
        let b_summary = StagedOutput::create(&summary, b"B summary\n").expect("stage B summary");
        let b_report = StagedOutput::create(&report, b"B report\n").expect("stage B report");
        let (a_reached_tx, a_reached_rx) = mpsc::channel();
        let (release_a_tx, release_a_rx) = mpsc::channel();
        let (b_done_tx, b_done_rx) = mpsc::channel();

        let a = thread::spawn(move || {
            publish_pair_with_hook(a_summary, a_report, || {
                a_reached_tx.send(()).expect("signal A summary");
                release_a_rx.recv().expect("release A report");
            })
        });
        a_reached_rx.recv().expect("A published its summary");
        let competing_lock = open_private_lock_file(
            &output_lock_path(&summary).expect("derive competing summary lock"),
        )
        .expect("open competing summary lock");
        assert!(
            matches!(
                competing_lock
                    .try_lock()
                    .expect_err("A still holds the summary lock"),
                std::fs::TryLockError::WouldBlock
            ),
            "the competing lock must be blocked by A"
        );
        let b = thread::spawn(move || {
            let result = publish_pair(b_summary, b_report);
            b_done_tx.send(result).expect("signal B completion");
        });

        let early_b_result = b_done_rx.recv_timeout(Duration::from_millis(100)).ok();
        let b_was_blocked = early_b_result.is_none();
        release_a_tx.send(()).expect("release A");
        a.join().expect("join A").expect("publish A pair");
        let b_result = match early_b_result {
            Some(result) => result,
            None => b_done_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("B completes after A"),
        };
        b_result.expect("publish B pair");
        b.join().expect("join B");

        assert!(
            b_was_blocked,
            "B must wait until A has published both artifacts"
        );
        assert_eq!(
            fs::read_to_string(&summary).expect("read summary"),
            "B summary\n"
        );
        assert_eq!(
            fs::read_to_string(&report).expect("read report"),
            "B report\n"
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn publication_lock_rejects_a_symlink_without_touching_its_target() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-lock-symlink-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create test directory");
        let summary = directory.join("summary.json");
        let report = directory.join("report.md");
        let target = directory.join("unrelated");
        fs::write(&target, "unrelated\n").expect("write lock target");
        let summary_lock = output_lock_path(&summary).expect("derive lock path");
        symlink(&target, &summary_lock).expect("create lock symlink");

        let error = match OutputLocks::acquire(&summary, &report) {
            Ok(_) => panic!("symlink lock must be rejected"),
            Err(error) => error,
        };

        assert!(error.contains("lock path is not a regular file"));
        assert_eq!(
            fs::read_to_string(&target).expect("read lock target"),
            "unrelated\n"
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn publication_lock_names_remain_within_common_filesystem_limits() {
        let destination = PathBuf::from("/tmp").join(format!("{}.json", "x".repeat(240)));

        let lock = output_lock_path(&destination).expect("derive lock path");

        assert!(
            lock.file_name()
                .expect("lock file name")
                .to_string_lossy()
                .len()
                <= 128
        );
    }
}
