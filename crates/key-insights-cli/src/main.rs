use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::atomic::{AtomicU64, Ordering},
};

use key_insights::{analyze_jsonl, render_markdown, render_summary_json};
use serde::{Deserialize, Serialize};

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
    recover_outputs(&paths.summary, &paths.report)?;

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
    #[cfg(test)]
    fn capture(destination: &Path) -> Result<Self, String> {
        match fs::symlink_metadata(destination) {
            Ok(metadata) if metadata.is_file() => {
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
            _ => Self::capture_at(destination, unused_sibling_name(destination, "backup")?),
        }
    }

    fn capture_at(destination: &Path, backup_path: PathBuf) -> Result<Self, String> {
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
                match link_without_replacement(destination, &backup_path) {
                    Ok(()) => {}
                    Err(error)
                        if error.kind() == std::io::ErrorKind::AlreadyExists
                            && same_file(destination, &backup_path) => {}
                    Err(error) => {
                        return Err(format!(
                            "failed to link existing output {} into a backup: {error}",
                            destination.display()
                        ));
                    }
                }
                if let Err(error) = sync_parent_directory(destination) {
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
            let restore_path = link_to_unused_sibling(backup_path, "restore")?;
            if let Err(error) = fs::rename(&restore_path, &self.destination) {
                let _ = fs::remove_file(&restore_path);
                return Err(format!(
                    "failed to restore previous output {}: {error}",
                    self.destination.display()
                ));
            }
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

    fn discard(&mut self) -> Result<(), String> {
        if let Some(backup_path) = &self.backup_path {
            fs::remove_file(backup_path).map_err(|error| {
                format!(
                    "failed to remove publication backup {}: {error}",
                    backup_path.display()
                )
            })?;
            sync_parent_directory(backup_path)
                .map_err(|error| format!("failed to sync publication backup cleanup: {error}"))?;
            self.backup_path = None;
        }
        Ok(())
    }
}

struct PairRecoveryPaths {
    active: PathBuf,
    committed: PathBuf,
    rollback: PathBuf,
    summary_backup: PathBuf,
    report_backup: PathBuf,
    summary_index: PathBuf,
    report_index: PathBuf,
}

impl PairRecoveryPaths {
    fn new(summary: &Path, report: &Path) -> Result<Self, String> {
        let summary_parent = summary
            .parent()
            .ok_or_else(|| format!("output path has no parent: {}", summary.display()))?;
        let report_parent = report
            .parent()
            .ok_or_else(|| format!("output path has no parent: {}", report.display()))?;
        let identifier = pair_identifier(summary, report);
        Ok(Self {
            active: summary_parent.join(format!(".key-insights.pair-{identifier}.active")),
            committed: summary_parent.join(format!(".key-insights.pair-{identifier}.committed")),
            rollback: summary_parent.join(format!(".key-insights.pair-{identifier}.rollback")),
            summary_backup: summary_parent
                .join(format!(".key-insights.pair-{identifier}.summary.backup")),
            report_backup: report_parent
                .join(format!(".key-insights.pair-{identifier}.report.backup")),
            summary_index: output_recovery_index_path(summary)?,
            report_index: output_recovery_index_path(report)?,
        })
    }
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum RecoveryRole {
    Summary,
    Report,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DestinationRecoveryIndex {
    version: u8,
    pair_identifier: String,
    role: RecoveryRole,
    previous_output: bool,
    destination: String,
    journal_parent: String,
    peer_index: String,
}

struct PairPublication {
    paths: PairRecoveryPaths,
    summary_backup: OutputBackup,
    report_backup: OutputBackup,
}

fn recover_outputs(summary: &Path, report: &Path) -> Result<(), String> {
    let _locks = OutputLocks::acquire(summary, report)?;
    recover_destination(summary)?;
    recover_destination(report)?;
    let paths = PairRecoveryPaths::new(summary, report)?;
    recover_pair(summary, report, &paths)
}

impl PairPublication {
    fn begin(summary: &Path, report: &Path) -> Result<Self, String> {
        let paths = PairRecoveryPaths::new(summary, report)?;
        recover_destination(summary)?;
        recover_destination(report)?;
        recover_pair(summary, report, &paths)?;
        let mut summary_backup = OutputBackup::capture_at(summary, paths.summary_backup.clone())?;
        let report_backup = match OutputBackup::capture_at(report, paths.report_backup.clone()) {
            Ok(backup) => backup,
            Err(error) => return Err(with_rollback(error, [&mut summary_backup])),
        };
        let previous_outputs = [
            summary_backup.backup_path.is_some(),
            report_backup.backup_path.is_some(),
        ];
        if let Err(error) =
            create_destination_recovery_indexes(summary, report, &paths, previous_outputs)
        {
            let mut report_backup = report_backup;
            return Err(with_rollback(
                error,
                [&mut summary_backup, &mut report_backup],
            ));
        }
        if let Err(error) = create_recovery_marker(&paths.active, previous_outputs) {
            let mut report_backup = report_backup;
            let error = with_rollback(error, [&mut summary_backup, &mut report_backup]);
            let _ = remove_destination_recovery_indexes(&paths);
            return Err(error);
        }
        if let Err(error) = sync_parent_directory(&paths.active) {
            let mut report_backup = report_backup;
            let error = format!("failed to sync active publication marker: {error}");
            let rollback = with_rollback(error, [&mut summary_backup, &mut report_backup]);
            let _ = remove_recovery_marker(&paths.active);
            let _ = remove_destination_recovery_indexes(&paths);
            return Err(rollback);
        }
        Ok(Self {
            paths,
            summary_backup,
            report_backup,
        })
    }

    fn rollback(mut self, error: String) -> String {
        if let Err(marker_error) =
            transition_recovery_marker(&self.paths.active, &self.paths.rollback, "rollback")
        {
            return format!("{error}; failed to commit rollback decision: {marker_error}");
        }
        if let Err(rollback_error) =
            restore_backups([&mut self.summary_backup, &mut self.report_backup])
        {
            return format!("{error}; rollback failed: {rollback_error}");
        }
        if let Err(index_error) = remove_destination_recovery_indexes(&self.paths) {
            return format!("{error}; failed to remove recovery indexes: {index_error}");
        }
        if let Err(cleanup_error) =
            discard_backups([&mut self.summary_backup, &mut self.report_backup])
        {
            return format!("{error}; failed to clean rollback backups: {cleanup_error}");
        }
        if let Err(marker_error) = remove_recovery_marker(&self.paths.rollback) {
            return format!("{error}; failed to remove rollback marker: {marker_error}");
        }
        error
    }

    fn commit(mut self) -> Result<(), String> {
        link_without_replacement(&self.paths.active, &self.paths.committed)
            .map_err(|error| format!("failed to commit publication recovery marker: {error}"))?;
        sync_parent_directory(&self.paths.committed)
            .map_err(|error| format!("failed to sync committed publication marker: {error}"))?;
        fs::remove_file(&self.paths.active)
            .map_err(|error| format!("failed to retire active publication marker: {error}"))?;
        sync_parent_directory(&self.paths.committed)
            .map_err(|error| format!("failed to sync retired publication marker: {error}"))?;

        let summary = self.summary_backup.destination.clone();
        let report = self.report_backup.destination.clone();
        self.summary_backup.discard()?;
        self.report_backup.discard()?;
        sync_output_directories(&summary, &report)?;
        remove_destination_recovery_indexes(&self.paths)?;
        remove_recovery_marker(&self.paths.committed)
            .map_err(|error| format!("failed to remove committed publication marker: {error}"))
    }
}

fn create_destination_recovery_indexes(
    summary: &Path,
    report: &Path,
    paths: &PairRecoveryPaths,
    previous_outputs: [bool; 2],
) -> Result<(), String> {
    let journal_parent = paths
        .active
        .parent()
        .ok_or_else(|| "publication journal has no parent".to_owned())?;
    let pair_identifier = pair_identifier(summary, report);
    let summary_index = DestinationRecoveryIndex {
        version: 1,
        pair_identifier: pair_identifier.clone(),
        role: RecoveryRole::Summary,
        previous_output: previous_outputs[0],
        destination: encode_path(summary),
        journal_parent: encode_path(journal_parent),
        peer_index: encode_path(&paths.report_index),
    };
    let report_index = DestinationRecoveryIndex {
        version: 1,
        pair_identifier,
        role: RecoveryRole::Report,
        previous_output: previous_outputs[1],
        destination: encode_path(report),
        journal_parent: encode_path(journal_parent),
        peer_index: encode_path(&paths.summary_index),
    };
    write_destination_recovery_index(&paths.summary_index, &summary_index)?;
    if let Err(error) = write_destination_recovery_index(&paths.report_index, &report_index) {
        let _ = remove_file_and_sync(&paths.summary_index);
        return Err(error);
    }
    Ok(())
}

fn write_destination_recovery_index(
    path: &Path,
    index: &DestinationRecoveryIndex,
) -> Result<(), String> {
    let contents = serde_json::to_vec(index)
        .map_err(|error| format!("failed to encode destination recovery index: {error}"))?;
    let mut file = open_private_new_file(path).map_err(|error| {
        format!(
            "failed to create destination recovery index {}: {error}",
            path.display()
        )
    })?;
    file.write_all(&contents)
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            format!(
                "failed to persist destination recovery index {}: {error}",
                path.display()
            )
        })?;
    sync_parent_directory(path).map_err(|error| {
        format!(
            "failed to sync destination recovery index {}: {error}",
            path.display()
        )
    })
}

fn recover_destination(destination: &Path) -> Result<(), String> {
    recover_destination_with_hook(destination, || {})
}

fn recover_destination_with_hook<F>(
    destination: &Path,
    after_destination_cleanup: F,
) -> Result<(), String>
where
    F: FnOnce(),
{
    let index_path = output_recovery_index_path(destination)?;
    let Some(index) = read_destination_recovery_index(&index_path)? else {
        return Ok(());
    };
    validate_pair_identifier(&index.pair_identifier)?;
    if index.version != 1 {
        return Err(format!(
            "unsupported destination recovery index version: {}",
            index.version
        ));
    }
    if decode_path(&index.destination)? != destination {
        return Err("destination recovery index belongs to a different output".to_owned());
    }
    let journal_parent = decode_path(&index.journal_parent)?;
    let peer_index = decode_path(&index.peer_index)?;
    let active = journal_parent.join(format!(
        ".key-insights.pair-{}.active",
        index.pair_identifier
    ));
    let committed = journal_parent.join(format!(
        ".key-insights.pair-{}.committed",
        index.pair_identifier
    ));
    let rollback = journal_parent.join(format!(
        ".key-insights.pair-{}.rollback",
        index.pair_identifier
    ));
    let role = match index.role {
        RecoveryRole::Summary => "summary",
        RecoveryRole::Report => "report",
    };
    let backup = destination
        .parent()
        .ok_or_else(|| format!("output path has no parent: {}", destination.display()))?
        .join(format!(
            ".key-insights.pair-{}.{}.backup",
            index.pair_identifier, role
        ));
    let active_state = read_recovery_marker(&active)?;
    let committed_state = read_recovery_marker(&committed)?;
    let rollback_state = read_recovery_marker(&rollback)?;
    if committed_state.is_some() && rollback_state.is_some() {
        return Err("transaction has conflicting commit and rollback markers".to_owned());
    }
    if let (Some(active_state), Some(committed_state)) = (active_state, committed_state)
        && (active_state != committed_state || !same_file(&active, &committed))
    {
        return Err("destination recovery markers do not describe one transaction".to_owned());
    }
    if let (Some(active_state), Some(rollback_state)) = (active_state, rollback_state)
        && (active_state != rollback_state || !same_file(&active, &rollback))
    {
        return Err("destination rollback markers do not describe one transaction".to_owned());
    }
    if let Some(marker_state) = active_state.or(committed_state).or(rollback_state) {
        let marker_previous_output = match index.role {
            RecoveryRole::Summary => marker_state[0],
            RecoveryRole::Report => marker_state[1],
        };
        if marker_previous_output != index.previous_output {
            return Err("destination recovery index disagrees with its transaction".to_owned());
        }
    }

    let rollback_selected =
        rollback_state.is_some() || (active_state.is_some() && committed_state.is_none());
    if rollback_selected {
        if rollback_state.is_none() {
            transition_recovery_marker(&active, &rollback, "rollback")?;
        }
        let mut output_backup = OutputBackup {
            destination: destination.to_owned(),
            backup_path: recovery_backup(&backup, index.previous_output)?,
        };
        output_backup.restore().map_err(|error| {
            format!(
                "failed to recover interrupted output {}: {error}",
                destination.display()
            )
        })?;
        remove_file_and_sync(&index_path)?;
        output_backup.discard()?;
    } else if committed_state.is_some() {
        validate_committed_backup(&backup, index.previous_output)?;
        remove_regular_file_if_present(&backup)?;
        sync_parent_directory(&backup)
            .map_err(|error| format!("failed to sync committed backup cleanup: {error}"))?;
        remove_file_and_sync(&index_path)?;
    } else {
        match existing_regular_file(&backup)? {
            Some(_) if same_file(destination, &backup) => {
                remove_file_and_sync(&backup)?;
            }
            Some(_) => {
                return Err(format!(
                    "uncommitted destination backup blocks recovery: {}",
                    backup.display()
                ));
            }
            None => {}
        }
        remove_file_and_sync(&index_path)?;
    }

    after_destination_cleanup();
    let peer_uses_same_transaction = read_destination_recovery_index(&peer_index)?
        .is_some_and(|peer| peer.pair_identifier == index.pair_identifier);
    if !peer_uses_same_transaction {
        remove_file_if_present_and_sync(&active)?;
        remove_file_if_present_and_sync(&committed)?;
        remove_file_if_present_and_sync(&rollback)?;
    }
    Ok(())
}

fn read_destination_recovery_index(
    path: &Path,
) -> Result<Option<DestinationRecoveryIndex>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            return Err(format!(
                "destination recovery index is not a regular file: {}",
                path.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed to inspect destination recovery index {}: {error}",
                path.display()
            ));
        }
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|error| {
        format!(
            "failed to open destination recovery index {}: {error}",
            path.display()
        )
    })?;
    if !open_recovery_index_matches_path(&file, path).map_err(|error| {
        format!(
            "failed to verify destination recovery index {}: {error}",
            path.display()
        )
    })? {
        return Err(format!(
            "destination recovery index changed while opening it: {}",
            path.display()
        ));
    }
    let mut contents = Vec::new();
    file.take(65_537)
        .read_to_end(&mut contents)
        .map_err(|error| format!("failed to read destination recovery index: {error}"))?;
    if contents.len() > 65_536 {
        return Err("destination recovery index exceeds 64 KiB".to_owned());
    }
    serde_json::from_slice(&contents)
        .map(Some)
        .map_err(|error| format!("invalid destination recovery index: {error}"))
}

fn remove_destination_recovery_indexes(paths: &PairRecoveryPaths) -> Result<(), String> {
    remove_file_if_present_and_sync(&paths.summary_index)?;
    remove_file_if_present_and_sync(&paths.report_index)
}

fn remove_file_and_sync(path: &Path) -> Result<(), String> {
    fs::remove_file(path)
        .map_err(|error| format!("failed to remove {}: {error}", path.display()))?;
    sync_parent_directory(path)
        .map_err(|error| format!("failed to sync cleanup for {}: {error}", path.display()))
}

fn remove_file_if_present_and_sync(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(metadata) if metadata.is_file() => remove_file_and_sync(path),
        Ok(_) => Err(format!(
            "refusing to remove non-regular file: {}",
            path.display()
        )),
        Err(error) => Err(format!("failed to inspect {}: {error}", path.display())),
    }
}

fn validate_pair_identifier(identifier: &str) -> Result<(), String> {
    if identifier.len() == 32 && identifier.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("destination recovery index has an invalid pair identifier".to_owned())
    }
}

fn output_recovery_index_path(destination: &Path) -> Result<PathBuf, String> {
    let parent = destination
        .parent()
        .ok_or_else(|| format!("output path has no parent: {}", destination.display()))?;
    Ok(parent.join(format!(
        ".key-insights.output-{}.recovery",
        path_identifier(destination)
    )))
}

fn encode_path(path: &Path) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = path_bytes(path);
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_path(encoded: &str) -> Result<PathBuf, String> {
    if !encoded.len().is_multiple_of(2) || encoded.len() > 16_384 {
        return Err("destination recovery index contains an invalid path".to_owned());
    }
    let mut bytes = Vec::with_capacity(encoded.len() / 2);
    for pair in encoded.as_bytes().chunks_exact(2) {
        let high = decode_hex_digit(pair[0])?;
        let low = decode_hex_digit(pair[1])?;
        bytes.push((high << 4) | low);
    }
    path_from_bytes(bytes)
}

fn decode_hex_digit(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err("destination recovery index contains invalid hex".to_owned()),
    }
}

#[cfg(unix)]
fn path_from_bytes(bytes: Vec<u8>) -> Result<PathBuf, String> {
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: Vec<u8>) -> Result<PathBuf, String> {
    String::from_utf8(bytes)
        .map(PathBuf::from)
        .map_err(|_| "destination recovery index path is not UTF-8".to_owned())
}

#[cfg(unix)]
fn open_recovery_index_matches_path(file: &File, path: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let open = file.metadata()?;
    let path = fs::metadata(path)?;
    Ok(open.dev() == path.dev()
        && open.ino() == path.ino()
        && open.nlink() == 1
        && open.mode() & 0o077 == 0)
}

#[cfg(not(unix))]
fn open_recovery_index_matches_path(_file: &File, path: &Path) -> std::io::Result<bool> {
    Ok(fs::metadata(path)?.is_file())
}

fn recover_pair(summary: &Path, report: &Path, paths: &PairRecoveryPaths) -> Result<(), String> {
    let active = read_recovery_marker(&paths.active)?;
    let committed = read_recovery_marker(&paths.committed)?;
    let rollback = read_recovery_marker(&paths.rollback)?;
    if committed.is_some() && rollback.is_some() {
        return Err("transaction has conflicting commit and rollback markers".to_owned());
    }
    if let (Some(active_state), Some(committed_state)) = (active, committed)
        && (active_state != committed_state || !same_file(&paths.active, &paths.committed))
    {
        return Err("publication recovery markers do not describe one transaction".to_owned());
    }
    if let (Some(active_state), Some(rollback_state)) = (active, rollback)
        && (active_state != rollback_state || !same_file(&paths.active, &paths.rollback))
    {
        return Err("publication rollback markers do not describe one transaction".to_owned());
    }
    if rollback.is_some() {
        remove_regular_file_if_present(&paths.summary_backup)?;
        remove_regular_file_if_present(&paths.report_backup)?;
        sync_output_directories(summary, report)?;
        if active.is_some() {
            remove_recovery_marker(&paths.active)?;
        }
        remove_recovery_marker(&paths.rollback)?;
    } else if let (Some(previous_outputs), None) = (active, committed) {
        let mut summary_backup = OutputBackup {
            destination: summary.to_owned(),
            backup_path: recovery_backup(&paths.summary_backup, previous_outputs[0])?,
        };
        let mut report_backup = OutputBackup {
            destination: report.to_owned(),
            backup_path: recovery_backup(&paths.report_backup, previous_outputs[1])?,
        };
        restore_backups([&mut summary_backup, &mut report_backup]).map_err(|error| {
            format!("failed to recover interrupted paired publication: {error}")
        })?;
        remove_recovery_marker(&paths.active)
            .map_err(|error| format!("failed to remove recovered publication marker: {error}"))?;
        discard_backups([&mut summary_backup, &mut report_backup])
            .map_err(|error| format!("failed to clean recovered publication backups: {error}"))?;
    } else if let Some(previous_outputs) = committed {
        validate_committed_backup(&paths.summary_backup, previous_outputs[0])?;
        validate_committed_backup(&paths.report_backup, previous_outputs[1])?;
        remove_regular_file_if_present(&paths.summary_backup)?;
        remove_regular_file_if_present(&paths.report_backup)?;
        sync_output_directories(summary, report)?;
        if active.is_some() {
            remove_recovery_marker(&paths.active)?;
        }
        remove_recovery_marker(&paths.committed)?;
    } else {
        reject_unowned_backup(summary, &paths.summary_backup)?;
        reject_unowned_backup(report, &paths.report_backup)?;
    }
    Ok(())
}

fn recovery_backup(path: &Path, expected: bool) -> Result<Option<PathBuf>, String> {
    let backup = existing_regular_file(path)?;
    match (expected, backup) {
        (true, Some(path)) => Ok(Some(path)),
        (false, None) => Ok(None),
        (true, None) => Err(format!(
            "required publication backup is missing: {}",
            path.display()
        )),
        (false, Some(_)) => Err(format!(
            "unexpected publication backup blocks recovery: {}",
            path.display()
        )),
    }
}

fn validate_committed_backup(path: &Path, expected: bool) -> Result<(), String> {
    let backup = existing_regular_file(path)?;
    if !expected && backup.is_some() {
        return Err(format!(
            "unexpected publication backup blocks committed cleanup: {}",
            path.display()
        ));
    }
    Ok(())
}

fn reject_unowned_backup(destination: &Path, backup: &Path) -> Result<(), String> {
    match fs::symlink_metadata(backup) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(metadata) if metadata.is_file() && same_file(destination, backup) => {
            fs::remove_file(backup).map_err(|error| {
                format!(
                    "failed to remove incomplete backup {}: {error}",
                    backup.display()
                )
            })?;
            sync_parent_directory(backup)
                .map_err(|error| format!("failed to sync incomplete backup cleanup: {error}"))
        }
        Ok(_) => Err(format!(
            "unowned publication backup blocks recovery: {}",
            backup.display()
        )),
        Err(error) => Err(format!(
            "failed to inspect publication backup {}: {error}",
            backup.display()
        )),
    }
}

fn existing_regular_file(path: &Path) -> Result<Option<PathBuf>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(Some(path.to_owned())),
        Ok(_) => Err(format!(
            "publication backup is not a regular file: {}",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "failed to inspect publication backup {}: {error}",
            path.display()
        )),
    }
}

fn remove_regular_file_if_present(path: &Path) -> Result<(), String> {
    if existing_regular_file(path)?.is_some() {
        fs::remove_file(path).map_err(|error| {
            format!(
                "failed to remove publication backup {}: {error}",
                path.display()
            )
        })?;
    }
    Ok(())
}

fn create_recovery_marker(path: &Path, previous_outputs: [bool; 2]) -> Result<(), String> {
    let mut file = open_private_new_file(path)
        .map_err(|error| format!("failed to create publication recovery marker: {error}"))?;
    let contents = [
        if previous_outputs[0] { b'1' } else { b'0' },
        if previous_outputs[1] { b'1' } else { b'0' },
        b'\n',
    ];
    file.write_all(&contents)
        .map_err(|error| format!("failed to write publication recovery marker: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("failed to sync publication recovery marker: {error}"))
}

fn transition_recovery_marker(
    source: &Path,
    destination: &Path,
    label: &str,
) -> Result<(), String> {
    match link_without_replacement(source, destination) {
        Ok(()) => {}
        Err(error)
            if error.kind() == std::io::ErrorKind::AlreadyExists
                && same_file(source, destination) => {}
        Err(error) => {
            return Err(format!(
                "failed to create {label} marker {}: {error}",
                destination.display()
            ));
        }
    }
    sync_parent_directory(destination)
        .map_err(|error| format!("failed to sync {label} marker: {error}"))?;
    match fs::symlink_metadata(source) {
        Ok(metadata) if metadata.is_file() && same_file(source, destination) => {
            fs::remove_file(source)
                .map_err(|error| format!("failed to retire active marker: {error}"))?;
            sync_parent_directory(destination)
                .map_err(|error| format!("failed to sync retired active marker: {error}"))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => return Err("active marker changed during state transition".to_owned()),
        Err(error) => return Err(format!("failed to inspect active marker: {error}")),
    }
    Ok(())
}

fn read_recovery_marker(path: &Path) -> Result<Option<[bool; 2]>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => Err(format!(
            "publication recovery marker is not a regular file: {}",
            path.display()
        ))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed to inspect publication recovery marker {}: {error}",
                path.display()
            ));
        }
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|error| {
        format!(
            "failed to open publication recovery marker {}: {error}",
            path.display()
        )
    })?;
    if !open_recovery_marker_matches_path(&file, path).map_err(|error| {
        format!(
            "failed to verify publication recovery marker {}: {error}",
            path.display()
        )
    })? {
        return Err(format!(
            "publication recovery marker changed while opening it: {}",
            path.display()
        ));
    }
    let mut contents = Vec::new();
    file.take(4).read_to_end(&mut contents).map_err(|error| {
        format!(
            "failed to read publication recovery marker {}: {error}",
            path.display()
        )
    })?;
    match contents.as_slice() {
        [summary @ (b'0' | b'1'), report @ (b'0' | b'1'), b'\n'] => {
            Ok(Some([*summary == b'1', *report == b'1']))
        }
        _ => Err(format!(
            "publication recovery marker is invalid: {}",
            path.display()
        )),
    }
}

#[cfg(unix)]
fn open_recovery_marker_matches_path(file: &File, path: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let open = file.metadata()?;
    let path = fs::metadata(path)?;
    Ok(open.dev() == path.dev()
        && open.ino() == path.ino()
        && matches!(open.nlink(), 1 | 2)
        && open.mode() & 0o077 == 0)
}

#[cfg(not(unix))]
fn open_recovery_marker_matches_path(_file: &File, path: &Path) -> std::io::Result<bool> {
    Ok(fs::metadata(path)?.is_file())
}

fn remove_recovery_marker(path: &Path) -> Result<(), String> {
    fs::remove_file(path)
        .map_err(|error| format!("failed to remove {}: {error}", path.display()))?;
    sync_parent_directory(path).map_err(|error| format!("failed to sync marker directory: {error}"))
}

fn pair_identifier(summary: &Path, report: &Path) -> String {
    identifier_for_paths(&[summary, report])
}

fn path_identifier(path: &Path) -> String {
    identifier_for_paths(&[path])
}

fn identifier_for_paths(paths: &[&Path]) -> String {
    let mut left = 0xcbf29ce484222325_u64;
    let mut right = 0x84222325cbf29ce4_u64;
    for path in paths {
        let bytes = path_bytes(path);
        for byte in (bytes.len() as u64).to_le_bytes().into_iter().chain(bytes) {
            left = (left ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            right = (right ^ u64::from(byte)).wrapping_mul(0x9e3779b185ebca87);
        }
    }
    format!("{left:016x}{right:016x}")
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().to_string_lossy().into_owned().into_bytes()
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
    let publication = PairPublication::begin(&summary.destination, &report.destination)?;

    if let Err(error) = summary.publish() {
        return Err(publication.rollback(error));
    }
    after_summary();
    if let Err(error) = report.publish() {
        return Err(publication.rollback(error));
    }
    if let Err(error) = sync_output_directories(
        &publication.summary_backup.destination,
        &publication.report_backup.destination,
    ) {
        return Err(publication.rollback(error));
    }

    publication.commit()
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
    let mut backups = backups;
    let rollback_errors: Vec<_> = backups
        .iter_mut()
        .filter_map(|backup| backup.restore().err())
        .collect();
    if !rollback_errors.is_empty() {
        return format!("{error}; rollback failed: {}", rollback_errors.join("; "));
    }
    let cleanup_errors: Vec<_> = backups
        .iter_mut()
        .filter_map(|backup| backup.discard().err())
        .collect();
    if cleanup_errors.is_empty() {
        error
    } else {
        format!(
            "{error}; rollback cleanup failed: {}",
            cleanup_errors.join("; ")
        )
    }
}

fn restore_backups<const N: usize>(backups: [&mut OutputBackup; N]) -> Result<(), String> {
    let rollback_errors: Vec<_> = backups
        .into_iter()
        .filter_map(|backup| backup.restore().err())
        .collect();
    if rollback_errors.is_empty() {
        Ok(())
    } else {
        Err(rollback_errors.join("; "))
    }
}

fn discard_backups<const N: usize>(backups: [&mut OutputBackup; N]) -> Result<(), String> {
    let cleanup_errors: Vec<_> = backups
        .into_iter()
        .filter_map(|backup| backup.discard().err())
        .collect();
    if cleanup_errors.is_empty() {
        Ok(())
    } else {
        Err(cleanup_errors.join("; "))
    }
}

#[cfg(test)]
fn unused_sibling_name(destination: &Path, label: &str) -> Result<PathBuf, String> {
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
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
            Ok(_) => continue,
            Err(error) => return Err(format!("failed to inspect backup candidate: {error}")),
        }
    }
    Err(format!(
        "failed to reserve a backup path for {}",
        destination.display()
    ))
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
        let directory = fs::canonicalize(directory).expect("canonicalize test directory");
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
    fn next_publication_recovers_a_pair_interrupted_after_the_summary() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-interrupted-pair-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create test directory");
        let directory = fs::canonicalize(directory).expect("canonicalize test directory");
        let summary = directory.join("summary.json");
        let report = directory.join("report.md");
        fs::write(&summary, "old summary\n").expect("write old summary");
        fs::write(&report, "old report\n").expect("write old report");
        let interrupted_summary =
            StagedOutput::create(&summary, b"interrupted summary\n").expect("stage summary");
        let interrupted_report =
            StagedOutput::create(&report, b"interrupted report\n").expect("stage report");

        let interrupted = catch_unwind(AssertUnwindSafe(|| {
            publish_pair_with_hook(interrupted_summary, interrupted_report, || {
                panic!("simulate process termination after summary publication");
            })
        }));
        assert!(interrupted.is_err(), "publication must be interrupted");
        assert_eq!(
            fs::read_to_string(&summary).expect("read interrupted summary"),
            "interrupted summary\n"
        );
        assert_eq!(
            fs::read_to_string(&report).expect("read old report"),
            "old report\n"
        );

        let empty_input = directory.join("empty.jsonl");
        fs::write(&empty_input, "").expect("write invalid retry input");
        let retry_error = super::run(vec![
            "analyze".into(),
            empty_input.into_os_string(),
            "--summary".into(),
            summary.clone().into_os_string(),
            "--report".into(),
            report.clone().into_os_string(),
        ])
        .expect_err("analysis must fail after startup recovery");
        assert!(
            retry_error.contains("session"),
            "recovery must complete before validation: {retry_error}"
        );

        assert_eq!(
            fs::read_to_string(&summary).expect("read recovered summary"),
            "old summary\n"
        );
        assert_eq!(
            fs::read_to_string(&report).expect("read recovered report"),
            "old report\n"
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn interrupted_publication_recovers_outputs_that_were_previously_absent() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-interrupted-absent-pair-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create test directory");
        let summary = directory.join("summary.json");
        let report = directory.join("report.md");

        let interrupted_summary =
            StagedOutput::create(&summary, b"interrupted summary\n").expect("stage summary");
        let interrupted_report =
            StagedOutput::create(&report, b"interrupted report\n").expect("stage report");
        let interrupted = catch_unwind(AssertUnwindSafe(|| {
            publish_pair_with_hook(interrupted_summary, interrupted_report, || {
                panic!("simulate process termination after summary publication");
            })
        }));
        assert!(interrupted.is_err(), "publication must be interrupted");

        let retry_summary =
            StagedOutput::create(&summary, b"retry summary\n").expect("stage retry summary");
        let retry_report =
            StagedOutput::create(&report, b"retry report\n").expect("stage retry report");
        fs::remove_file(
            retry_summary
                .temporary_path
                .as_ref()
                .expect("retry summary temporary path"),
        )
        .expect("force retry publication failure");
        publish_pair(retry_summary, retry_report).expect_err("retry must fail after recovery");

        assert!(!summary.exists(), "previously absent summary stays absent");
        assert!(!report.exists(), "previously absent report stays absent");
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn committed_recovery_keeps_the_new_pair() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-committed-pair-{}-{unique}",
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
        let _locks = OutputLocks::acquire(&summary, &report).expect("acquire locks");
        let publication = super::PairPublication::begin(&summary, &report).expect("begin pair");
        summary_output.publish().expect("publish summary");
        report_output.publish().expect("publish report");
        super::sync_output_directories(&summary, &report).expect("sync pair");
        link_without_replacement(&publication.paths.active, &publication.paths.committed)
            .expect("install committed marker");
        super::sync_parent_directory(&publication.paths.committed).expect("sync marker");
        drop(publication);
        drop(_locks);

        let retry_summary =
            StagedOutput::create(&summary, b"retry summary\n").expect("stage retry summary");
        let retry_report =
            StagedOutput::create(&report, b"retry report\n").expect("stage retry report");
        fs::remove_file(
            retry_summary
                .temporary_path
                .as_ref()
                .expect("retry summary temporary path"),
        )
        .expect("force retry publication failure");
        publish_pair(retry_summary, retry_report).expect_err("retry must fail after recovery");

        assert_eq!(
            fs::read_to_string(&summary).expect("read committed summary"),
            "new summary\n"
        );
        assert_eq!(
            fs::read_to_string(&report).expect("read committed report"),
            "new report\n"
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn rollback_recovery_is_idempotent_after_one_output_was_restored() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-partial-recovery-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create test directory");
        let summary = directory.join("summary.json");
        let report = directory.join("report.md");
        fs::write(&summary, "old summary\n").expect("write old summary");
        fs::write(&report, "old report\n").expect("write old report");
        let summary_output =
            StagedOutput::create(&summary, b"new summary\n").expect("stage summary");
        let _locks = OutputLocks::acquire(&summary, &report).expect("acquire locks");
        let mut publication = super::PairPublication::begin(&summary, &report).expect("begin pair");
        summary_output.publish().expect("publish summary");
        publication
            .summary_backup
            .restore()
            .expect("partially restore summary");
        drop(publication);
        drop(_locks);

        super::recover_outputs(&summary, &report).expect("repeat interrupted rollback");

        assert_eq!(
            fs::read_to_string(&summary).expect("read restored summary"),
            "old summary\n"
        );
        assert_eq!(
            fs::read_to_string(&report).expect("read restored report"),
            "old report\n"
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn reusing_one_destination_recovers_its_previous_transaction_first() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-reused-destination-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create test directory");
        let summary = directory.join("summary.json");
        let report_a = directory.join("report-a.md");
        let report_b = directory.join("report-b.md");
        fs::write(&summary, "old summary\n").expect("write old summary");
        fs::write(&report_a, "old report A\n").expect("write old report A");

        let interrupted_summary =
            StagedOutput::create(&summary, b"interrupted summary A\n").expect("stage summary A");
        let interrupted_report =
            StagedOutput::create(&report_a, b"interrupted report A\n").expect("stage report A");
        let interrupted = catch_unwind(AssertUnwindSafe(|| {
            publish_pair_with_hook(interrupted_summary, interrupted_report, || {
                panic!("simulate process termination after summary A");
            })
        }));
        assert!(interrupted.is_err(), "publication A must be interrupted");

        publish_pair(
            StagedOutput::create(&summary, b"summary B\n").expect("stage summary B"),
            StagedOutput::create(&report_b, b"report B\n").expect("stage report B"),
        )
        .expect("publish pair B after recovering summary A");
        super::recover_outputs(&summary, &report_a).expect("recover stale pair A state");

        assert_eq!(
            fs::read_to_string(&summary).expect("read summary B"),
            "summary B\n",
            "stale pair A must not revert the newer successful summary"
        );
        assert_eq!(
            fs::read_to_string(&report_b).expect("read report B"),
            "report B\n"
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn recovery_restarts_after_the_last_destination_cleanup() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-recovery-cleanup-crash-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create test directory");
        let summary = directory.join("summary.json");
        let report = directory.join("report.md");
        fs::write(&summary, "old summary\n").expect("write old summary");
        fs::write(&report, "old report\n").expect("write old report");

        let interrupted = catch_unwind(AssertUnwindSafe(|| {
            publish_pair_with_hook(
                StagedOutput::create(&summary, b"new summary\n").expect("stage summary"),
                StagedOutput::create(&report, b"new report\n").expect("stage report"),
                || panic!("interrupt publication after summary"),
            )
        }));
        assert!(interrupted.is_err(), "publication must be interrupted");
        super::recover_destination(&summary).expect("recover first destination");

        let cleanup_interrupted = catch_unwind(AssertUnwindSafe(|| {
            super::recover_destination_with_hook(&report, || {
                panic!("interrupt after final destination cleanup");
            })
        }));
        assert!(
            cleanup_interrupted.is_err(),
            "recovery cleanup must be interrupted"
        );

        super::recover_outputs(&summary, &report).expect("restart recovery cleanup");
        assert_eq!(
            fs::read_to_string(&summary).expect("read recovered summary"),
            "old summary\n"
        );
        assert_eq!(
            fs::read_to_string(&report).expect("read recovered report"),
            "old report\n"
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn recovery_restarts_after_the_first_destination_cleanup() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-first-recovery-cleanup-crash-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create test directory");
        let summary = directory.join("summary.json");
        let report = directory.join("report.md");
        fs::write(&summary, "old summary\n").expect("write old summary");
        fs::write(&report, "old report\n").expect("write old report");

        let interrupted = catch_unwind(AssertUnwindSafe(|| {
            publish_pair_with_hook(
                StagedOutput::create(&summary, b"new summary\n").expect("stage summary"),
                StagedOutput::create(&report, b"new report\n").expect("stage report"),
                || panic!("interrupt publication after summary"),
            )
        }));
        assert!(interrupted.is_err(), "publication must be interrupted");

        let cleanup_interrupted = catch_unwind(AssertUnwindSafe(|| {
            super::recover_destination_with_hook(&summary, || {
                panic!("interrupt after first destination cleanup");
            })
        }));
        assert!(
            cleanup_interrupted.is_err(),
            "recovery cleanup must be interrupted"
        );

        super::recover_outputs(&summary, &report).expect("restart recovery cleanup");
        assert_eq!(
            fs::read_to_string(&summary).expect("read recovered summary"),
            "old summary\n"
        );
        assert_eq!(
            fs::read_to_string(&report).expect("read recovered report"),
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

        let mut backup = OutputBackup::capture(&destination).expect("capture output");

        assert_eq!(
            fs::read_to_string(&destination).expect("public output remains available"),
            "previous summary\n"
        );
        backup.discard().expect("discard backup");
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
