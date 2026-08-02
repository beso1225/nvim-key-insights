use super::*;

pub(super) struct PairRecoveryPaths {
    pub(super) active: PathBuf,
    pub(super) committed: PathBuf,
    pub(super) rollback: PathBuf,
    pub(super) summary_backup: PathBuf,
    pub(super) report_backup: PathBuf,
    pub(super) summary_index: PathBuf,
    pub(super) report_index: PathBuf,
}

impl PairRecoveryPaths {
    pub(super) fn new(summary: &Path, report: &Path) -> Result<Self, String> {
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

pub(super) fn reject_recovery_artifact_collisions(
    summary: &Path,
    report: &Path,
) -> Result<(), String> {
    let paths = PairRecoveryPaths::new(summary, report)?;
    let artifacts = [
        paths.active.as_path(),
        paths.committed.as_path(),
        paths.rollback.as_path(),
        paths.summary_backup.as_path(),
        paths.report_backup.as_path(),
        paths.summary_index.as_path(),
        paths.report_index.as_path(),
    ];
    for output in [summary, report] {
        for artifact in artifacts {
            if output_names_may_collide(output, artifact)? {
                return Err(format!(
                    "output path must not collide with a recovery artifact: {}",
                    output.display()
                ));
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RecoveryRole {
    Summary,
    Report,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DestinationRecoveryIndex {
    version: u8,
    pair_identifier: String,
    role: RecoveryRole,
    previous_output: bool,
    destination: String,
    journal_parent: String,
    peer_index: String,
}

pub(super) struct PairPublication {
    pub(super) paths: PairRecoveryPaths,
    pub(super) summary_backup: OutputBackup,
    pub(super) report_backup: OutputBackup,
    pub(super) directories: Option<[Arc<ResolvedDirectory>; 2]>,
}

#[cfg(test)]
pub(super) fn recover_outputs(summary: &Path, report: &Path) -> Result<(), String> {
    let _locks = OutputLocks::acquire(summary, report)?;
    recover_destination(summary)?;
    recover_destination(report)?;
    let paths = PairRecoveryPaths::new(summary, report)?;
    recover_pair(summary, report, &paths)
}

pub(super) fn recover_outputs_anchored(
    summary: &ResolvedOutputPath,
    report: &ResolvedOutputPath,
) -> Result<(), String> {
    recover_outputs_anchored_with_scavenger(
        summary,
        report,
        current_unix_time_seconds()?,
        staged_output_process_is_alive,
    )
}

pub(super) fn recover_outputs_anchored_with_scavenger<F>(
    summary: &ResolvedOutputPath,
    report: &ResolvedOutputPath,
    now_seconds: u64,
    mut process_is_alive: F,
) -> Result<(), String>
where
    F: FnMut(u32) -> bool,
{
    summary.directory.verify_current()?;
    report.directory.verify_current()?;
    let _locks = OutputLocks::acquire_anchored(summary, report)?;
    let outputs = [summary, report];
    recover_destination_anchored(summary, outputs)?;
    summary.directory.verify_current()?;
    recover_destination_anchored(report, outputs)?;
    report.directory.verify_current()?;
    let paths = PairRecoveryPaths::new(summary.as_path(), report.as_path())?;
    recover_pair_anchored(summary, report, &paths)?;
    summary.directory.verify_current()?;
    report.directory.verify_current()?;
    scavenge_staged_outputs(summary, now_seconds, &mut process_is_alive)?;
    if same_file(&summary.directory.path, &report.directory.path) {
        Ok(())
    } else {
        scavenge_staged_outputs(report, now_seconds, &mut process_is_alive)
    }
}

pub(super) fn read_recovery_index_anchored(
    directory: &ResolvedDirectory,
    name: &std::ffi::OsStr,
) -> Result<Option<DestinationRecoveryIndex>, String> {
    let Some(file) = directory
        .open_read_file(name)
        .map_err(|error| format!("failed to open destination recovery index: {error}"))?
    else {
        return Ok(None);
    };
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect destination recovery index: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 || metadata.mode() & 0o077 != 0 {
            return Err("destination recovery index has unsafe metadata".to_owned());
        }
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

pub(super) fn read_recovery_marker_anchored(
    directory: &ResolvedDirectory,
    name: &std::ffi::OsStr,
) -> Result<Option<[bool; 2]>, String> {
    let Some(file) = directory
        .open_read_file(name)
        .map_err(|error| format!("failed to open publication recovery marker: {error}"))?
    else {
        return Ok(None);
    };
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect publication recovery marker: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if !matches!(metadata.nlink(), 1 | 2) || metadata.mode() & 0o077 != 0 {
            return Err("publication recovery marker has unsafe metadata".to_owned());
        }
    }
    let mut contents = Vec::new();
    file.take(4)
        .read_to_end(&mut contents)
        .map_err(|error| format!("failed to read publication recovery marker: {error}"))?;
    match contents.as_slice() {
        [summary @ (b'0' | b'1'), report @ (b'0' | b'1'), b'\n'] => {
            Ok(Some([*summary == b'1', *report == b'1']))
        }
        _ => Err("publication recovery marker is invalid".to_owned()),
    }
}

pub(super) fn open_recorded_directory(
    path: &Path,
    known_outputs: [&ResolvedOutputPath; 2],
) -> Result<Arc<ResolvedDirectory>, String> {
    if let Some(output) = known_outputs.into_iter().find(|output| {
        output.directory.path == path || output.path.parent().is_some_and(|parent| parent == path)
    }) {
        return Ok(Arc::clone(&output.directory));
    }
    let resolved = fs::canonicalize(path).map_err(|error| {
        format!(
            "failed to resolve recovery directory {}: {error}",
            path.display()
        )
    })?;
    Ok(Arc::new(ResolvedDirectory::open(&resolved)?))
}

pub(super) fn open_optional_recorded_directory(
    path: &Path,
    known_outputs: [&ResolvedOutputPath; 2],
) -> Result<Option<Arc<ResolvedDirectory>>, String> {
    if let Some(output) = known_outputs.into_iter().find(|output| {
        output.directory.path == path || output.path.parent().is_some_and(|parent| parent == path)
    }) {
        return Ok(Some(Arc::clone(&output.directory)));
    }
    match fs::canonicalize(path) {
        Ok(resolved) => ResolvedDirectory::open(&resolved).map(Arc::new).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "failed to resolve recovery directory {}: {error}",
            path.display()
        )),
    }
}

pub(super) fn anchored_child_same_file(
    directory: &ResolvedDirectory,
    left: &std::ffi::OsStr,
    right: &std::ffi::OsStr,
) -> Result<bool, String> {
    let left = directory
        .child_metadata(left)
        .map_err(|error| format!("failed to inspect recovery artifact: {error}"))?;
    let right = directory
        .child_metadata(right)
        .map_err(|error| format!("failed to inspect recovery artifact: {error}"))?;
    Ok(left.is_some() && left == right)
}

pub(super) fn anchored_recovery_backup(
    output: &ResolvedOutputPath,
    backup_name: std::ffi::OsString,
    expected: bool,
) -> Result<OutputBackup, String> {
    let backup_metadata = output
        .directory
        .child_metadata(&backup_name)
        .map_err(|error| format!("failed to inspect publication backup: {error}"))?;
    if backup_metadata.is_some_and(|metadata| !metadata.is_regular_file()) {
        return Err("publication backup is not a regular file".to_owned());
    }
    let backup_name = match (expected, backup_metadata.is_some()) {
        (true, true) => Some(backup_name),
        (false, false) => None,
        (true, false) => return Err("required publication backup is missing".to_owned()),
        (false, true) => return Err("unexpected publication backup blocks recovery".to_owned()),
    };
    Ok(OutputBackup {
        destination: output.path.clone(),
        backup_path: backup_name
            .as_ref()
            .map(|name| output.directory.path.join(name)),
        anchored: Some(AnchoredBackup {
            directory: Arc::clone(&output.directory),
            destination_name: output
                .path
                .file_name()
                .expect("resolved output name")
                .to_owned(),
            backup_name,
            expected_destination: backup_metadata,
        }),
    })
}

pub(super) fn recover_destination_anchored(
    output: &ResolvedOutputPath,
    known_outputs: [&ResolvedOutputPath; 2],
) -> Result<(), String> {
    recover_destination_anchored_with_hook(output, known_outputs, || {})
}

pub(super) fn recover_destination_anchored_with_hook<F>(
    output: &ResolvedOutputPath,
    known_outputs: [&ResolvedOutputPath; 2],
    after_index_read: F,
) -> Result<(), String>
where
    F: FnOnce(),
{
    let index_path = output_recovery_index_path(output.as_path())?;
    let index_name = index_path.file_name().expect("recovery index name");
    let Some(index) = read_recovery_index_anchored(&output.directory, index_name)? else {
        return Ok(());
    };
    validate_pair_identifier(&index.pair_identifier)?;
    if index.version != 1 {
        return Err(format!(
            "unsupported destination recovery index version: {}",
            index.version
        ));
    }
    if decode_path(&index.destination)? != output.path {
        return Err("destination recovery index belongs to a different output".to_owned());
    }
    after_index_read();

    let journal_parent = decode_path(&index.journal_parent)?;
    let journal_directory = open_recorded_directory(&journal_parent, known_outputs)?;
    let peer_index = decode_path(&index.peer_index)?;
    let peer_parent = peer_index
        .parent()
        .ok_or_else(|| "peer recovery index has no parent".to_owned())?;
    let peer_directory = open_optional_recorded_directory(peer_parent, known_outputs)?;
    let active_name: std::ffi::OsString =
        format!(".key-insights.pair-{}.active", index.pair_identifier).into();
    let committed_name: std::ffi::OsString =
        format!(".key-insights.pair-{}.committed", index.pair_identifier).into();
    let rollback_name: std::ffi::OsString =
        format!(".key-insights.pair-{}.rollback", index.pair_identifier).into();
    let backup_role = match index.role {
        RecoveryRole::Summary => "summary",
        RecoveryRole::Report => "report",
    };
    let backup_name: std::ffi::OsString = format!(
        ".key-insights.pair-{}.{}.backup",
        index.pair_identifier, backup_role
    )
    .into();

    let active = read_recovery_marker_anchored(&journal_directory, &active_name)?;
    let committed = read_recovery_marker_anchored(&journal_directory, &committed_name)?;
    let rollback = read_recovery_marker_anchored(&journal_directory, &rollback_name)?;
    if committed.is_some() && rollback.is_some() {
        return Err("transaction has conflicting commit and rollback markers".to_owned());
    }
    if let (Some(active_state), Some(committed_state)) = (active, committed)
        && (active_state != committed_state
            || !anchored_child_same_file(&journal_directory, &active_name, &committed_name)?)
    {
        return Err("destination recovery markers do not describe one transaction".to_owned());
    }
    if let (Some(active_state), Some(rollback_state)) = (active, rollback)
        && (active_state != rollback_state
            || !anchored_child_same_file(&journal_directory, &active_name, &rollback_name)?)
    {
        return Err("destination rollback markers do not describe one transaction".to_owned());
    }
    if let Some(marker_state) = active.or(committed).or(rollback) {
        let marker_previous_output = match index.role {
            RecoveryRole::Summary => marker_state[0],
            RecoveryRole::Report => marker_state[1],
        };
        if marker_previous_output != index.previous_output {
            return Err("destination recovery index disagrees with its transaction".to_owned());
        }
    }

    let rollback_selected = rollback.is_some() || (active.is_some() && committed.is_none());
    if rollback_selected {
        if rollback.is_none() {
            transition_child_marker(&journal_directory, &active_name, &rollback_name)?;
        }
        let mut backup =
            anchored_recovery_backup(output, backup_name.clone(), index.previous_output)?;
        backup.restore().map_err(|error| {
            format!(
                "failed to recover interrupted output {}: {error}",
                output.path.display()
            )
        })?;
        remove_anchored_file(&output.directory, index_name)?;
        backup.discard()?;
    } else if committed.is_some() {
        let backup = output
            .directory
            .child_metadata(&backup_name)
            .map_err(|error| format!("failed to inspect committed backup: {error}"))?;
        if !index.previous_output && backup.is_some() {
            return Err("unexpected publication backup blocks committed cleanup".to_owned());
        }
        if backup.is_some() {
            remove_anchored_file(&output.directory, &backup_name)?;
        }
        remove_anchored_file(&output.directory, index_name)?;
    } else {
        let backup = output
            .directory
            .child_metadata(&backup_name)
            .map_err(|error| format!("failed to inspect uncommitted backup: {error}"))?;
        if backup.is_some() {
            let destination_name = output.path.file_name().expect("resolved output name");
            if !anchored_child_same_file(&output.directory, destination_name, &backup_name)? {
                return Err("uncommitted destination backup blocks recovery".to_owned());
            }
            remove_anchored_file(&output.directory, &backup_name)?;
        }
        remove_anchored_file(&output.directory, index_name)?;
    }

    let peer_uses_same_transaction = match &peer_directory {
        Some(directory) => read_recovery_index_anchored(
            directory,
            peer_index.file_name().expect("peer recovery index name"),
        )?
        .is_some_and(|peer| peer.pair_identifier == index.pair_identifier),
        None => false,
    };
    if !peer_uses_same_transaction {
        remove_anchored_file(&journal_directory, &active_name)?;
        remove_anchored_file(&journal_directory, &committed_name)?;
        remove_anchored_file(&journal_directory, &rollback_name)?;
    }
    output.directory.verify_current()?;
    journal_directory.verify_current()?;
    if let Some(directory) = peer_directory {
        directory.verify_current()?;
    }
    Ok(())
}

pub(super) fn recover_pair_anchored(
    summary: &ResolvedOutputPath,
    report: &ResolvedOutputPath,
    paths: &PairRecoveryPaths,
) -> Result<(), String> {
    let active_name = paths.active.file_name().expect("active marker name");
    let committed_name = paths.committed.file_name().expect("committed marker name");
    let rollback_name = paths.rollback.file_name().expect("rollback marker name");
    let summary_backup_name = paths
        .summary_backup
        .file_name()
        .expect("summary backup name")
        .to_owned();
    let report_backup_name = paths
        .report_backup
        .file_name()
        .expect("report backup name")
        .to_owned();
    let active = read_recovery_marker_anchored(&summary.directory, active_name)?;
    let committed = read_recovery_marker_anchored(&summary.directory, committed_name)?;
    let rollback = read_recovery_marker_anchored(&summary.directory, rollback_name)?;
    if committed.is_some() && rollback.is_some() {
        return Err("transaction has conflicting commit and rollback markers".to_owned());
    }
    if let (Some(active_state), Some(committed_state)) = (active, committed)
        && (active_state != committed_state
            || !anchored_child_same_file(&summary.directory, active_name, committed_name)?)
    {
        return Err("publication recovery markers do not describe one transaction".to_owned());
    }
    if let (Some(active_state), Some(rollback_state)) = (active, rollback)
        && (active_state != rollback_state
            || !anchored_child_same_file(&summary.directory, active_name, rollback_name)?)
    {
        return Err("publication rollback markers do not describe one transaction".to_owned());
    }

    if rollback.is_some() {
        remove_anchored_file(&summary.directory, &summary_backup_name)?;
        remove_anchored_file(&report.directory, &report_backup_name)?;
        if active.is_some() {
            remove_anchored_file(&summary.directory, active_name)?;
        }
        remove_anchored_file(&summary.directory, rollback_name)?;
    } else if let (Some(previous_outputs), None) = (active, committed) {
        let mut summary_backup =
            anchored_recovery_backup(summary, summary_backup_name, previous_outputs[0])?;
        let mut report_backup =
            anchored_recovery_backup(report, report_backup_name, previous_outputs[1])?;
        restore_backups([&mut summary_backup, &mut report_backup]).map_err(|error| {
            format!("failed to recover interrupted paired publication: {error}")
        })?;
        remove_anchored_file(&summary.directory, active_name)?;
        discard_backups([&mut summary_backup, &mut report_backup])
            .map_err(|error| format!("failed to clean recovered publication backups: {error}"))?;
    } else if let Some(previous_outputs) = committed {
        for (directory, name, expected) in [
            (
                &summary.directory,
                &summary_backup_name,
                previous_outputs[0],
            ),
            (&report.directory, &report_backup_name, previous_outputs[1]),
        ] {
            let backup = directory
                .child_metadata(name)
                .map_err(|error| format!("failed to inspect committed backup: {error}"))?;
            if !expected && backup.is_some() {
                return Err("unexpected publication backup blocks committed cleanup".to_owned());
            }
            if backup.is_some() {
                remove_anchored_file(directory, name)?;
            }
        }
        if active.is_some() {
            remove_anchored_file(&summary.directory, active_name)?;
        }
        remove_anchored_file(&summary.directory, committed_name)?;
    } else {
        for (output, backup_name) in [
            (summary, summary_backup_name.as_os_str()),
            (report, report_backup_name.as_os_str()),
        ] {
            if output
                .directory
                .child_metadata(backup_name)
                .map_err(|error| format!("failed to inspect publication backup: {error}"))?
                .is_some()
            {
                let destination_name = output.path.file_name().expect("resolved output name");
                if !anchored_child_same_file(&output.directory, destination_name, backup_name)? {
                    return Err("unowned publication backup blocks recovery".to_owned());
                }
                remove_anchored_file(&output.directory, backup_name)?;
            }
        }
    }
    summary.directory.verify_current()?;
    report.directory.verify_current()
}

impl PairPublication {
    #[cfg(test)]
    pub(super) fn begin(summary: &Path, report: &Path) -> Result<Self, String> {
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
            directories: None,
        })
    }

    pub(super) fn begin_anchored(
        summary: &StagedOutput,
        report: &StagedOutput,
    ) -> Result<Self, String> {
        let paths = PairRecoveryPaths::new(&summary.destination, &report.destination)?;
        let resolved_summary = ResolvedOutputPath {
            path: summary.destination.clone(),
            directory: Arc::clone(&summary.directory),
        };
        let resolved_report = ResolvedOutputPath {
            path: report.destination.clone(),
            directory: Arc::clone(&report.directory),
        };
        let outputs = [&resolved_summary, &resolved_report];
        recover_destination_anchored(&resolved_summary, outputs)?;
        recover_destination_anchored(&resolved_report, outputs)?;
        recover_pair_anchored(&resolved_summary, &resolved_report, &paths)?;
        let mut summary_backup =
            OutputBackup::capture_anchored(summary, paths.summary_backup.clone())?;
        let report_backup =
            match OutputBackup::capture_anchored(report, paths.report_backup.clone()) {
                Ok(backup) => backup,
                Err(error) => return Err(with_rollback(error, [&mut summary_backup])),
            };
        let previous_outputs = [
            summary_backup.backup_path.is_some(),
            report_backup.backup_path.is_some(),
        ];
        if let Err(error) = create_destination_recovery_indexes_anchored(
            &summary.destination,
            &report.destination,
            &paths,
            previous_outputs,
            [&summary.directory, &report.directory],
        ) {
            let mut report_backup = report_backup;
            return Err(with_rollback(
                error,
                [&mut summary_backup, &mut report_backup],
            ));
        }
        let marker_contents = [
            if previous_outputs[0] { b'1' } else { b'0' },
            if previous_outputs[1] { b'1' } else { b'0' },
            b'\n',
        ];
        if let Err(error) = publish_private_sidecar_anchored(
            &summary.directory,
            paths.active.file_name().expect("active marker name"),
            &marker_contents,
        ) {
            let mut report_backup = report_backup;
            let error = with_rollback(error, [&mut summary_backup, &mut report_backup]);
            let _ = remove_anchored_file(
                &summary.directory,
                paths.summary_index.file_name().expect("summary index name"),
            );
            let _ = remove_anchored_file(
                &report.directory,
                paths.report_index.file_name().expect("report index name"),
            );
            return Err(error);
        }
        Ok(Self {
            paths,
            summary_backup,
            report_backup,
            directories: Some([
                Arc::clone(&summary.directory),
                Arc::clone(&report.directory),
            ]),
        })
    }

    pub(super) fn rollback(mut self, error: String) -> String {
        if self.directories.is_some() {
            return self.rollback_anchored(error);
        }
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

    pub(super) fn abort_preserving_destinations(self, error: String) -> String {
        match self.commit() {
            Ok(()) => error,
            Err(cleanup_error) => {
                format!("{error}; failed to clean aborted publication: {cleanup_error}")
            }
        }
    }

    pub(super) fn commit(mut self) -> Result<(), String> {
        if self.directories.is_some() {
            return self.commit_anchored();
        }
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

    pub(super) fn rollback_anchored(&mut self, error: String) -> String {
        let directories = self.directories.as_ref().expect("anchored publication");
        if let Err(marker_error) = transition_child_marker(
            &directories[0],
            self.paths.active.file_name().expect("active marker name"),
            self.paths
                .rollback
                .file_name()
                .expect("rollback marker name"),
        ) {
            return format!("{error}; failed to commit rollback decision: {marker_error}");
        }
        if let Err(rollback_error) =
            restore_backups([&mut self.summary_backup, &mut self.report_backup])
        {
            return format!("{error}; rollback failed: {rollback_error}");
        }
        if let Err(index_error) = remove_anchored_recovery_indexes(&self.paths, directories) {
            return format!("{error}; failed to remove recovery indexes: {index_error}");
        }
        if let Err(cleanup_error) =
            discard_backups([&mut self.summary_backup, &mut self.report_backup])
        {
            return format!("{error}; failed to clean rollback backups: {cleanup_error}");
        }
        if let Err(marker_error) = remove_anchored_file(
            &directories[0],
            self.paths
                .rollback
                .file_name()
                .expect("rollback marker name"),
        ) {
            return format!("{error}; failed to remove rollback marker: {marker_error}");
        }
        error
    }

    pub(super) fn commit_anchored(&mut self) -> Result<(), String> {
        let directories = self.directories.as_ref().expect("anchored publication");
        transition_child_marker(
            &directories[0],
            self.paths.active.file_name().expect("active marker name"),
            self.paths
                .committed
                .file_name()
                .expect("committed marker name"),
        )?;
        self.summary_backup.discard()?;
        self.report_backup.discard()?;
        directories[0]
            .sync()
            .map_err(|error| format!("failed to sync summary output directory: {error}"))?;
        directories[1]
            .sync()
            .map_err(|error| format!("failed to sync report output directory: {error}"))?;
        remove_anchored_recovery_indexes(&self.paths, directories)?;
        remove_anchored_file(
            &directories[0],
            self.paths
                .committed
                .file_name()
                .expect("committed marker name"),
        )
    }
}

pub(super) fn create_destination_recovery_indexes_anchored(
    summary: &Path,
    report: &Path,
    paths: &PairRecoveryPaths,
    previous_outputs: [bool; 2],
    directories: [&ResolvedDirectory; 2],
) -> Result<(), String> {
    let journal_parent = paths
        .active
        .parent()
        .ok_or_else(|| "publication journal has no parent".to_owned())?;
    let pair_identifier = pair_identifier(summary, report);
    let indexes = [
        DestinationRecoveryIndex {
            version: 1,
            pair_identifier: pair_identifier.clone(),
            role: RecoveryRole::Summary,
            previous_output: previous_outputs[0],
            destination: encode_path(summary),
            journal_parent: encode_path(journal_parent),
            peer_index: encode_path(&paths.report_index),
        },
        DestinationRecoveryIndex {
            version: 1,
            pair_identifier,
            role: RecoveryRole::Report,
            previous_output: previous_outputs[1],
            destination: encode_path(report),
            journal_parent: encode_path(journal_parent),
            peer_index: encode_path(&paths.summary_index),
        },
    ];
    let names = [
        paths.summary_index.file_name().expect("summary index name"),
        paths.report_index.file_name().expect("report index name"),
    ];
    for index in 0..2 {
        let contents = serde_json::to_vec(&indexes[index])
            .map_err(|error| format!("failed to encode destination recovery index: {error}"))?;
        if let Err(error) =
            publish_private_sidecar_anchored(directories[index], names[index], &contents)
        {
            if index == 1 {
                let _ = remove_anchored_file(directories[0], names[0]);
            }
            return Err(format!(
                "failed to publish destination recovery index: {error}"
            ));
        }
    }
    Ok(())
}

pub(super) fn publish_private_sidecar_anchored(
    directory: &ResolvedDirectory,
    name: &std::ffi::OsStr,
    contents: &[u8],
) -> Result<(), String> {
    let label = bounded_file_label(name);
    for _attempt in 0..100 {
        let temporary_name = unique_child_name(std::ffi::OsStr::new(&label), "sidecar-tmp");
        let mut file = match directory.open_private_new_file(&temporary_name) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("failed to create temporary sidecar: {error}")),
        };
        if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = directory.remove_child(&temporary_name);
            return Err(format!("failed to persist temporary sidecar: {error}"));
        }
        if let Err(error) = directory.link_child(&temporary_name, name) {
            drop(file);
            let _ = directory.remove_child(&temporary_name);
            return Err(format!("failed to publish sidecar: {error}"));
        }
        if let Err(error) = directory.remove_child(&temporary_name) {
            let published_is_ours = directory
                .open_file_has_child_identity(&file, name)
                .unwrap_or(false);
            drop(file);
            if published_is_ours {
                let _ = directory.remove_child(name);
            }
            let _ = directory.sync();
            return Err(format!("failed to retire temporary sidecar: {error}"));
        }
        if let Err(error) = directory.sync() {
            let published_is_ours = directory
                .open_file_has_child_identity(&file, name)
                .unwrap_or(false);
            drop(file);
            if published_is_ours {
                let _ = directory.remove_child(name);
            }
            let _ = directory.sync();
            return Err(format!("failed to sync published sidecar: {error}"));
        }
        return Ok(());
    }
    Err("failed to reserve a temporary sidecar".to_owned())
}

pub(super) fn transition_child_marker(
    directory: &ResolvedDirectory,
    source: &std::ffi::OsStr,
    destination: &std::ffi::OsStr,
) -> Result<(), String> {
    match directory.link_child(source, destination) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let source = directory
                .child_metadata(source)
                .map_err(|error| format!("failed to inspect active marker: {error}"))?;
            let destination = directory
                .child_metadata(destination)
                .map_err(|error| format!("failed to inspect recovery marker: {error}"))?;
            if source.is_none()
                || source != destination
                || source.is_some_and(|metadata| !metadata.is_regular_file())
            {
                return Err("recovery marker belongs to another transaction".to_owned());
            }
        }
        Err(error) => return Err(format!("failed to create recovery marker: {error}")),
    }
    directory
        .sync()
        .map_err(|error| format!("failed to sync recovery marker: {error}"))?;
    let source_metadata = directory
        .child_metadata(source)
        .map_err(|error| format!("failed to inspect active marker: {error}"))?;
    if let Some(source_metadata) = source_metadata {
        let destination_metadata = directory
            .child_metadata(destination)
            .map_err(|error| format!("failed to inspect recovery marker: {error}"))?;
        if !source_metadata.is_regular_file() || destination_metadata != Some(source_metadata) {
            return Err("active marker changed during state transition".to_owned());
        }
        directory
            .remove_child(source)
            .map_err(|error| format!("failed to retire active marker: {error}"))?;
        directory
            .sync()
            .map_err(|error| format!("failed to sync retired active marker: {error}"))?;
    }
    Ok(())
}

pub(super) fn remove_anchored_recovery_indexes(
    paths: &PairRecoveryPaths,
    directories: &[Arc<ResolvedDirectory>; 2],
) -> Result<(), String> {
    remove_anchored_file(
        &directories[0],
        paths.summary_index.file_name().expect("summary index name"),
    )?;
    remove_anchored_file(
        &directories[1],
        paths.report_index.file_name().expect("report index name"),
    )
}

pub(super) fn remove_anchored_file(
    directory: &ResolvedDirectory,
    name: &std::ffi::OsStr,
) -> Result<(), String> {
    match directory
        .child_metadata(name)
        .map_err(|error| format!("failed to inspect recovery artifact: {error}"))?
    {
        None => Ok(()),
        Some(metadata) if metadata.is_regular_file() => {
            directory
                .remove_child(name)
                .map_err(|error| format!("failed to remove recovery artifact: {error}"))?;
            directory
                .sync()
                .map_err(|error| format!("failed to sync recovery cleanup: {error}"))
        }
        Some(_) => Err("refusing to remove non-regular recovery artifact".to_owned()),
    }
}

#[cfg(test)]
pub(super) fn create_destination_recovery_indexes(
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

#[cfg(test)]
pub(super) fn write_destination_recovery_index(
    path: &Path,
    index: &DestinationRecoveryIndex,
) -> Result<(), String> {
    let contents = serde_json::to_vec(index)
        .map_err(|error| format!("failed to encode destination recovery index: {error}"))?;
    publish_private_sidecar(path, &contents)
        .map_err(|error| format!("failed to publish destination recovery index: {error}"))
}

#[cfg(test)]
pub(super) fn recover_destination(destination: &Path) -> Result<(), String> {
    recover_destination_with_hook(destination, || {})
}

#[cfg(test)]
pub(super) fn recover_destination_with_hook<F>(
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
            anchored: None,
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

#[cfg(test)]
pub(super) fn read_destination_recovery_index(
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

pub(super) fn remove_destination_recovery_indexes(paths: &PairRecoveryPaths) -> Result<(), String> {
    remove_file_if_present_and_sync(&paths.summary_index)?;
    remove_file_if_present_and_sync(&paths.report_index)
}

pub(super) fn remove_file_and_sync(path: &Path) -> Result<(), String> {
    fs::remove_file(path)
        .map_err(|error| format!("failed to remove {}: {error}", path.display()))?;
    sync_parent_directory(path)
        .map_err(|error| format!("failed to sync cleanup for {}: {error}", path.display()))
}

pub(super) fn remove_file_if_present_and_sync(path: &Path) -> Result<(), String> {
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

pub(super) fn validate_pair_identifier(identifier: &str) -> Result<(), String> {
    if identifier.len() == 32 && identifier.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("destination recovery index has an invalid pair identifier".to_owned())
    }
}

pub(super) fn output_recovery_index_path(destination: &Path) -> Result<PathBuf, String> {
    let parent = destination
        .parent()
        .ok_or_else(|| format!("output path has no parent: {}", destination.display()))?;
    Ok(parent.join(format!(
        ".key-insights.output-{}.recovery",
        path_identifier(destination)
    )))
}

pub(super) fn encode_path(path: &Path) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = path_bytes(path);
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

pub(super) fn decode_path(encoded: &str) -> Result<PathBuf, String> {
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

pub(super) fn decode_hex_digit(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err("destination recovery index contains invalid hex".to_owned()),
    }
}

#[cfg(unix)]
pub(super) fn path_from_bytes(bytes: Vec<u8>) -> Result<PathBuf, String> {
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

#[cfg(not(unix))]
pub(super) fn path_from_bytes(bytes: Vec<u8>) -> Result<PathBuf, String> {
    String::from_utf8(bytes)
        .map(PathBuf::from)
        .map_err(|_| "destination recovery index path is not UTF-8".to_owned())
}

#[cfg(all(test, unix))]
pub(super) fn open_recovery_index_matches_path(file: &File, path: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let open = file.metadata()?;
    let path = fs::metadata(path)?;
    Ok(open.dev() == path.dev()
        && open.ino() == path.ino()
        && open.nlink() == 1
        && open.mode() & 0o077 == 0)
}

#[cfg(all(test, not(unix)))]
pub(super) fn open_recovery_index_matches_path(_file: &File, path: &Path) -> std::io::Result<bool> {
    Ok(fs::metadata(path)?.is_file())
}

#[cfg(test)]
pub(super) fn recover_pair(
    summary: &Path,
    report: &Path,
    paths: &PairRecoveryPaths,
) -> Result<(), String> {
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
            anchored: None,
        };
        let mut report_backup = OutputBackup {
            destination: report.to_owned(),
            backup_path: recovery_backup(&paths.report_backup, previous_outputs[1])?,
            anchored: None,
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

#[cfg(test)]
pub(super) fn recovery_backup(path: &Path, expected: bool) -> Result<Option<PathBuf>, String> {
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

#[cfg(test)]
pub(super) fn validate_committed_backup(path: &Path, expected: bool) -> Result<(), String> {
    let backup = existing_regular_file(path)?;
    if !expected && backup.is_some() {
        return Err(format!(
            "unexpected publication backup blocks committed cleanup: {}",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn reject_unowned_backup(destination: &Path, backup: &Path) -> Result<(), String> {
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

#[cfg(test)]
pub(super) fn existing_regular_file(path: &Path) -> Result<Option<PathBuf>, String> {
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

#[cfg(test)]
pub(super) fn remove_regular_file_if_present(path: &Path) -> Result<(), String> {
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

#[cfg(test)]
pub(super) fn publish_private_sidecar(path: &Path, contents: &[u8]) -> Result<(), String> {
    publish_private_sidecar_with(path, contents, |file, contents| {
        file.write_all(contents)?;
        file.sync_all()
    })
}

#[cfg(test)]
pub(super) fn publish_private_sidecar_with<F>(
    path: &Path,
    contents: &[u8],
    persist: F,
) -> Result<(), String>
where
    F: FnOnce(&mut File, &[u8]) -> std::io::Result<()>,
{
    let parent = path
        .parent()
        .ok_or_else(|| format!("sidecar path has no parent: {}", path.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| format!("sidecar path has no file name: {}", path.display()))?;
    let label = bounded_file_label(name);
    let mut persist = Some(persist);
    for _attempt in 0..100 {
        let identifier = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temporary_path = parent.join(format!(
            ".key-insights.sidecar-{label}.tmp-{}-{identifier}",
            std::process::id()
        ));
        let mut file = match open_private_new_file(&temporary_path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "failed to create temporary sidecar for {}: {error}",
                    path.display()
                ));
            }
        };
        let persist = persist.take().expect("sidecar persistence runs once");
        if let Err(error) = persist(&mut file, contents) {
            drop(file);
            let _ = fs::remove_file(&temporary_path);
            return Err(format!("failed to persist temporary sidecar: {error}"));
        }
        if let Err(error) = rename_without_replacement(&temporary_path, path) {
            drop(file);
            let _ = fs::remove_file(&temporary_path);
            return Err(format!(
                "failed to publish sidecar {}: {error}",
                path.display()
            ));
        }
        if let Err(error) = sync_parent_directory(path) {
            let published_is_ours = open_file_matches_path(&file, path).unwrap_or(false);
            drop(file);
            if published_is_ours {
                let _ = fs::remove_file(path);
                let _ = sync_parent_directory(path);
            }
            return Err(format!(
                "failed to sync published sidecar {}: {error}",
                path.display()
            ));
        }
        return Ok(());
    }
    Err(format!(
        "failed to reserve a temporary sidecar for {}",
        path.display()
    ))
}

#[cfg(test)]
pub(super) fn create_recovery_marker(
    path: &Path,
    previous_outputs: [bool; 2],
) -> Result<(), String> {
    let contents = [
        if previous_outputs[0] { b'1' } else { b'0' },
        if previous_outputs[1] { b'1' } else { b'0' },
        b'\n',
    ];
    publish_private_sidecar(path, &contents)
        .map_err(|error| format!("failed to publish recovery marker: {error}"))
}

pub(super) fn transition_recovery_marker(
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

#[cfg(test)]
pub(super) fn read_recovery_marker(path: &Path) -> Result<Option<[bool; 2]>, String> {
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

#[cfg(all(test, unix))]
pub(super) fn open_recovery_marker_matches_path(file: &File, path: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let open = file.metadata()?;
    let path = fs::metadata(path)?;
    Ok(open.dev() == path.dev()
        && open.ino() == path.ino()
        && matches!(open.nlink(), 1 | 2)
        && open.mode() & 0o077 == 0)
}

#[cfg(all(test, not(unix)))]
pub(super) fn open_recovery_marker_matches_path(
    _file: &File,
    path: &Path,
) -> std::io::Result<bool> {
    Ok(fs::metadata(path)?.is_file())
}

pub(super) fn remove_recovery_marker(path: &Path) -> Result<(), String> {
    fs::remove_file(path)
        .map_err(|error| format!("failed to remove {}: {error}", path.display()))?;
    sync_parent_directory(path).map_err(|error| format!("failed to sync marker directory: {error}"))
}

pub(super) fn pair_identifier(summary: &Path, report: &Path) -> String {
    identifier_for_paths(&[summary, report])
}

pub(super) fn path_identifier(path: &Path) -> String {
    identifier_for_paths(&[path])
}

pub(super) fn identifier_for_paths(paths: &[&Path]) -> String {
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
pub(super) fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
pub(super) fn path_bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().to_string_lossy().into_owned().into_bytes()
}
