use super::*;

pub(super) fn same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    same_file_metadata(left, right)
}

pub(super) fn output_paths_may_collide(left: &Path, right: &Path) -> Result<bool, String> {
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
pub(super) fn same_file_metadata(left: &Path, right: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    match (fs::metadata(left), fs::metadata(right)) {
        (Ok(left), Ok(right)) => left.dev() == right.dev() && left.ino() == right.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
pub(super) fn same_file_metadata(_left: &Path, _right: &Path) -> bool {
    false
}

pub(super) struct NameProbeDirectory {
    pub(super) path: PathBuf,
    pub(super) files: Vec<PathBuf>,
}

impl NameProbeDirectory {
    pub(super) fn create(parent: &Path) -> Result<Self, String> {
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

pub(super) fn probe_name_collision(
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

pub(super) struct StagedOutput {
    pub(super) temporary_name: Option<std::ffi::OsString>,
    pub(super) destination: PathBuf,
    pub(super) destination_name: std::ffi::OsString,
    pub(super) directory: Arc<ResolvedDirectory>,
}

impl StagedOutput {
    pub(super) fn create<D: OutputDestination + ?Sized>(
        destination: &D,
        contents: &[u8],
    ) -> Result<Self, String> {
        let (destination, directory) = destination.resolve_destination()?;
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
            let temporary_name = temporary_path
                .file_name()
                .expect("temporary output has a file name");
            directory.verify_current()?;
            match directory.open_private_new_file(temporary_name) {
                Ok(mut file) => {
                    let staged = Self {
                        temporary_name: Some(temporary_name.to_owned()),
                        destination: destination.clone(),
                        destination_name: name.to_owned(),
                        directory: Arc::clone(&directory),
                    };
                    let write_result = file.write_all(contents).and_then(|()| file.sync_all());
                    drop(file);
                    write_result.map_err(|error| {
                        format!(
                            "failed to stage and sync {}: {error}",
                            staged.destination.display()
                        )
                    })?;
                    directory.verify_current()?;
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

    pub(super) fn publish(mut self) -> Result<(), String> {
        self.directory.verify_current()?;
        let temporary_name = self
            .temporary_name
            .take()
            .expect("staged output has a temporary name");
        if let Err(error) = self
            .directory
            .rename_child(&temporary_name, &self.destination_name)
        {
            self.temporary_name = Some(temporary_name);
            return Err(format!(
                "failed to publish {}: {error}",
                self.destination.display()
            ));
        }
        self.directory.verify_current()?;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn temporary_path(&self) -> PathBuf {
        self.directory.path.join(
            self.temporary_name
                .as_ref()
                .expect("staged output has a temporary name"),
        )
    }
}

impl Drop for StagedOutput {
    fn drop(&mut self) {
        if let Some(name) = &self.temporary_name {
            let _ = self.directory.remove_child(name);
        }
    }
}

pub(super) trait OutputDestination {
    fn resolve_destination(&self) -> Result<(PathBuf, Arc<ResolvedDirectory>), String>;
}

pub(super) trait AnchoredOutput {
    fn destination_path(&self) -> &Path;
    fn resolved_directory(&self) -> &ResolvedDirectory;
}

impl AnchoredOutput for ResolvedOutputPath {
    fn destination_path(&self) -> &Path {
        &self.path
    }

    fn resolved_directory(&self) -> &ResolvedDirectory {
        &self.directory
    }
}

impl AnchoredOutput for StagedOutput {
    fn destination_path(&self) -> &Path {
        &self.destination
    }

    fn resolved_directory(&self) -> &ResolvedDirectory {
        &self.directory
    }
}

impl OutputDestination for ResolvedOutputPath {
    fn resolve_destination(&self) -> Result<(PathBuf, Arc<ResolvedDirectory>), String> {
        Ok((self.path.clone(), Arc::clone(&self.directory)))
    }
}

impl OutputDestination for Path {
    fn resolve_destination(&self) -> Result<(PathBuf, Arc<ResolvedDirectory>), String> {
        let parent = self
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let parent = fs::canonicalize(parent)
            .map_err(|error| format!("failed to resolve parent {}: {error}", parent.display()))?;
        Ok((self.to_owned(), Arc::new(ResolvedDirectory::open(&parent)?)))
    }
}

impl OutputDestination for PathBuf {
    fn resolve_destination(&self) -> Result<(PathBuf, Arc<ResolvedDirectory>), String> {
        self.as_path().resolve_destination()
    }
}

pub(super) struct OutputBackup {
    pub(super) destination: PathBuf,
    pub(super) backup_path: Option<PathBuf>,
    pub(super) anchored: Option<AnchoredBackup>,
}

pub(super) struct AnchoredBackup {
    pub(super) directory: Arc<ResolvedDirectory>,
    pub(super) destination_name: std::ffi::OsString,
    pub(super) backup_name: Option<std::ffi::OsString>,
}

impl OutputBackup {
    #[cfg(test)]
    pub(super) fn capture(destination: &Path) -> Result<Self, String> {
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
                    anchored: None,
                })
            }
            _ => Self::capture_at(destination, unused_sibling_name(destination, "backup")?),
        }
    }

    #[cfg(test)]
    pub(super) fn capture_at(destination: &Path, backup_path: PathBuf) -> Result<Self, String> {
        match fs::symlink_metadata(destination) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                destination: destination.to_owned(),
                backup_path: None,
                anchored: None,
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
                    anchored: None,
                })
            }
            Err(error) => Err(format!(
                "failed to inspect output {} before publication: {error}",
                destination.display()
            )),
        }
    }

    pub(super) fn restore(&mut self) -> Result<(), String> {
        if self.anchored.is_some() {
            return self.restore_anchored();
        }
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

    pub(super) fn discard(&mut self) -> Result<(), String> {
        if self.anchored.is_some() {
            return self.discard_anchored();
        }
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

    pub(super) fn capture_anchored(
        output: &StagedOutput,
        backup_path: PathBuf,
    ) -> Result<Self, String> {
        Self::capture_anchored_with_hook(output, backup_path, || {})
    }

    pub(super) fn capture_anchored_with_hook<F>(
        output: &StagedOutput,
        backup_path: PathBuf,
        after_metadata: F,
    ) -> Result<Self, String>
    where
        F: FnOnce(),
    {
        let backup_name = backup_path
            .file_name()
            .ok_or_else(|| format!("backup path has no file name: {}", backup_path.display()))?
            .to_owned();
        let metadata = output
            .directory
            .child_metadata(&output.destination_name)
            .map_err(|error| {
                format!(
                    "failed to inspect output {} before publication: {error}",
                    output.destination.display()
                )
            })?;
        let backup_name = match metadata {
            None => None,
            Some(metadata) if !metadata.is_regular_file() => {
                return Err(format!(
                    "output destination became a non-regular file: {}",
                    output.destination.display()
                ));
            }
            Some(initial_metadata) => {
                after_metadata();
                output
                    .directory
                    .link_child(&output.destination_name, &backup_name)
                    .map_err(|error| {
                        format!(
                            "failed to link existing output {} into a backup: {error}",
                            output.destination.display()
                        )
                    })?;
                let source_metadata = output
                    .directory
                    .child_metadata(&output.destination_name)
                    .map_err(|error| {
                        format!("failed to recheck output after backup link: {error}")
                    })?;
                let backup_metadata = output
                    .directory
                    .child_metadata(&backup_name)
                    .map_err(|error| format!("failed to recheck publication backup: {error}"))?;
                if !source_metadata.is_some_and(|metadata| metadata.same_identity(initial_metadata))
                    || !backup_metadata
                        .is_some_and(|metadata| metadata.same_identity(initial_metadata))
                    || !initial_metadata.is_regular_file()
                {
                    let backup_is_owned = backup_metadata
                        .is_some_and(|metadata| metadata.same_identity(initial_metadata))
                        || (source_metadata.is_some() && source_metadata == backup_metadata);
                    if backup_is_owned {
                        let _ = output.directory.remove_child(&backup_name);
                        let _ = output.directory.sync();
                    }
                    return Err(format!(
                        "output {} changed while capturing its backup",
                        output.destination.display()
                    ));
                }
                if let Err(error) = output.directory.sync() {
                    if anchored_child_same_file(
                        &output.directory,
                        &output.destination_name,
                        &backup_name,
                    )
                    .unwrap_or(false)
                    {
                        let _ = output.directory.remove_child(&backup_name);
                        let _ = output.directory.sync();
                    }
                    return Err(format!(
                        "failed to sync backup for {}: {error}",
                        output.destination.display()
                    ));
                }
                Some(backup_name)
            }
        };
        Ok(Self {
            destination: output.destination.clone(),
            backup_path: backup_name.as_ref().map(|name| {
                output
                    .destination
                    .parent()
                    .expect("output has a parent")
                    .join(name)
            }),
            anchored: Some(AnchoredBackup {
                directory: Arc::clone(&output.directory),
                destination_name: output.destination_name.clone(),
                backup_name,
            }),
        })
    }

    pub(super) fn restore_anchored(&mut self) -> Result<(), String> {
        let anchored = self.anchored.as_mut().expect("anchored backup exists");
        if let Some(backup_name) = &anchored.backup_name {
            let backup_metadata = anchored
                .directory
                .child_metadata(backup_name)
                .map_err(|error| format!("failed to inspect publication backup: {error}"))?
                .ok_or_else(|| "publication backup disappeared during rollback".to_owned())?;
            let destination_metadata = anchored
                .directory
                .child_metadata(&anchored.destination_name)
                .map_err(|error| format!("failed to inspect output during rollback: {error}"))?;
            if destination_metadata == Some(backup_metadata) {
                return Ok(());
            }
            if destination_metadata.is_some_and(|metadata| !metadata.is_regular_file()) {
                return Err(format!(
                    "cannot restore output over non-regular file {}",
                    self.destination.display()
                ));
            }
            let restore_name = unique_child_name(&anchored.destination_name, "restore");
            anchored
                .directory
                .link_child(backup_name, &restore_name)
                .map_err(|error| format!("failed to stage restored output: {error}"))?;
            if let Err(error) = anchored
                .directory
                .rename_child(&restore_name, &anchored.destination_name)
            {
                let _ = anchored.directory.remove_child(&restore_name);
                return Err(format!("failed to restore previous output: {error}"));
            }
        } else if let Some(metadata) = anchored
            .directory
            .child_metadata(&anchored.destination_name)
            .map_err(|error| format!("failed to inspect output during rollback: {error}"))?
        {
            if !metadata.is_regular_file() {
                return Err(format!(
                    "cannot remove non-regular output during rollback: {}",
                    self.destination.display()
                ));
            }
            anchored
                .directory
                .remove_child(&anchored.destination_name)
                .map_err(|error| format!("failed to remove unpublished output: {error}"))?;
        }
        anchored
            .directory
            .sync()
            .map_err(|error| format!("failed to sync restored output: {error}"))
    }

    pub(super) fn discard_anchored(&mut self) -> Result<(), String> {
        let anchored = self.anchored.as_mut().expect("anchored backup exists");
        if let Some(backup_name) = anchored.backup_name.take() {
            anchored
                .directory
                .remove_child(&backup_name)
                .map_err(|error| format!("failed to remove publication backup: {error}"))?;
            anchored
                .directory
                .sync()
                .map_err(|error| format!("failed to sync publication backup cleanup: {error}"))?;
            self.backup_path = None;
        }
        Ok(())
    }
}

pub(super) fn unique_child_name(name: &std::ffi::OsStr, label: &str) -> std::ffi::OsString {
    let identifier = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        ".key-insights.{}.{label}-{}-{identifier}",
        bounded_file_label(name),
        std::process::id()
    )
    .into()
}

pub(super) fn publish_pair(summary: StagedOutput, report: StagedOutput) -> Result<(), String> {
    publish_pair_with_hook(summary, report, || {})
}

pub(super) fn publish_pair_with_hook<F>(
    summary: StagedOutput,
    report: StagedOutput,
    after_summary: F,
) -> Result<(), String>
where
    F: FnOnce(),
{
    summary.directory.verify_current()?;
    report.directory.verify_current()?;
    let summary_directory = Arc::clone(&summary.directory);
    let report_directory = Arc::clone(&report.directory);
    let _locks = OutputLocks::acquire_anchored(&summary, &report)?;
    let publication = PairPublication::begin_anchored(&summary, &report)?;

    if let Err(error) = summary.publish() {
        return Err(publication.rollback(error));
    }
    after_summary();
    if let Err(error) = report.publish() {
        return Err(publication.rollback(error));
    }
    let sync_result = summary_directory
        .sync()
        .and_then(|()| report_directory.sync())
        .map_err(|error| format!("failed to sync output directories: {error}"))
        .and_then(|()| summary_directory.verify_current())
        .and_then(|()| report_directory.verify_current());
    if let Err(error) = sync_result {
        return Err(publication.rollback(error));
    }

    publication.commit()
}

pub(super) fn sync_output_directories(summary: &Path, report: &Path) -> Result<(), String> {
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
pub(super) fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing parent"))?;
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
pub(super) fn sync_parent_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

pub(super) fn with_rollback<const N: usize>(
    error: String,
    backups: [&mut OutputBackup; N],
) -> String {
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

pub(super) fn restore_backups<const N: usize>(
    backups: [&mut OutputBackup; N],
) -> Result<(), String> {
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

pub(super) fn discard_backups<const N: usize>(
    backups: [&mut OutputBackup; N],
) -> Result<(), String> {
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
pub(super) fn unused_sibling_name(destination: &Path, label: &str) -> Result<PathBuf, String> {
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

pub(super) fn link_to_unused_sibling(destination: &Path, label: &str) -> Result<PathBuf, String> {
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

pub(super) fn link_without_replacement(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::hard_link(source, destination)
}

#[cfg(all(test, target_os = "linux"))]
pub(super) fn rename_without_replacement(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(test, target_os = "macos"))]
pub(super) fn rename_without_replacement(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let result =
        unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(test, windows))]
pub(super) fn rename_without_replacement(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(all(test, not(any(target_os = "linux", target_os = "macos", windows))))]
pub(super) fn rename_without_replacement(
    _source: &Path,
    _destination: &Path,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic no-replace rename is unavailable on this platform",
    ))
}

pub(super) struct OutputLocks {
    pub(super) _files: Vec<File>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct FilesystemIdentity {
    pub(super) device: u64,
    pub(super) inode: u64,
}

#[cfg(unix)]
pub(super) fn file_identity(file: &File) -> std::io::Result<FilesystemIdentity> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata()?;
    Ok(FilesystemIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(not(unix))]
pub(super) fn file_identity(_file: &File) -> std::io::Result<FilesystemIdentity> {
    Err(unsupported_directory_handle_operation())
}

pub(super) struct PreparedLock<'a> {
    pub(super) path: PathBuf,
    pub(super) name: std::ffi::OsString,
    pub(super) directory: &'a ResolvedDirectory,
    pub(super) file: File,
    pub(super) identity: FilesystemIdentity,
}

pub(super) fn prepare_anchored_locks<'a, S: AnchoredOutput, R: AnchoredOutput>(
    summary: &'a S,
    report: &'a R,
) -> Result<Vec<PreparedLock<'a>>, String> {
    struct Candidate<'a> {
        path: PathBuf,
        name: std::ffi::OsString,
        directory: &'a ResolvedDirectory,
    }

    let summary_path = output_lock_path(summary.destination_path())?;
    let report_path = output_lock_path(report.destination_path())?;
    let mut candidates = vec![
        Candidate {
            name: summary_path
                .file_name()
                .expect("summary lock name")
                .to_owned(),
            path: summary_path,
            directory: summary.resolved_directory(),
        },
        Candidate {
            name: report_path
                .file_name()
                .expect("report lock name")
                .to_owned(),
            path: report_path,
            directory: report.resolved_directory(),
        },
    ];
    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    if candidates[0].path == candidates[1].path {
        candidates.pop();
    }
    for candidate in &candidates {
        if output_paths_may_collide(&candidate.path, summary.destination_path())?
            || output_paths_may_collide(&candidate.path, report.destination_path())?
        {
            return Err(format!(
                "output path collides with publication lock: {}",
                candidate.path.display()
            ));
        }
    }

    let mut prepared: Vec<_> = candidates
        .into_iter()
        .map(|candidate| {
            let file = candidate
                .directory
                .open_private_lock_file(&candidate.name)
                .map_err(|error| {
                    format!(
                        "failed to open publication lock {}: {error}",
                        candidate.path.display()
                    )
                })?;
            if !candidate
                .directory
                .open_file_matches_child(&file, &candidate.name)
                .map_err(|error| {
                    format!(
                        "failed to verify publication lock {}: {error}",
                        candidate.path.display()
                    )
                })?
            {
                return Err(format!(
                    "publication lock changed while opening it: {}",
                    candidate.path.display()
                ));
            }
            let identity = file_identity(&file).map_err(|error| {
                format!(
                    "failed to identify publication lock {}: {error}",
                    candidate.path.display()
                )
            })?;
            Ok(PreparedLock {
                path: candidate.path,
                name: candidate.name,
                directory: candidate.directory,
                file,
                identity,
            })
        })
        .collect::<Result<_, _>>()?;
    prepared.sort_by_key(|lock| lock.identity);
    prepared.dedup_by_key(|lock| lock.identity);
    Ok(prepared)
}

impl OutputLocks {
    #[cfg(test)]
    pub(super) fn acquire(summary: &Path, report: &Path) -> Result<Self, String> {
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

        let mut prepared = Vec::with_capacity(lock_paths.len());
        for lock_path in lock_paths {
            let file = open_private_lock_file(&lock_path).map_err(|error| {
                format!(
                    "failed to open publication lock {}: {error}",
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
            let identity = file_identity(&file).map_err(|error| {
                format!(
                    "failed to identify publication lock {}: {error}",
                    lock_path.display()
                )
            })?;
            prepared.push((identity, lock_path, file));
        }
        prepared.sort_by_key(|(identity, _, _)| *identity);
        prepared.dedup_by_key(|(identity, _, _)| *identity);
        let mut files = Vec::with_capacity(prepared.len());
        for (_, lock_path, file) in prepared {
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

    pub(super) fn acquire_anchored<S: AnchoredOutput, R: AnchoredOutput>(
        summary: &S,
        report: &R,
    ) -> Result<Self, String> {
        Self::acquire_anchored_with_hook(summary, report, || {})
    }

    pub(super) fn acquire_anchored_with_hook<S: AnchoredOutput, R: AnchoredOutput, F>(
        summary: &S,
        report: &R,
        after_first_lock: F,
    ) -> Result<Self, String>
    where
        F: FnOnce(),
    {
        let prepared = prepare_anchored_locks(summary, report)?;
        let mut files = Vec::with_capacity(prepared.len());
        let mut hook = Some(after_first_lock);
        for prepared_lock in prepared {
            prepared_lock.file.lock().map_err(|error| {
                format!(
                    "failed to acquire publication lock {}: {error}",
                    prepared_lock.path.display()
                )
            })?;
            if !prepared_lock
                .directory
                .open_file_matches_child(&prepared_lock.file, &prepared_lock.name)
                .map_err(|error| {
                    format!(
                        "failed to verify publication lock {}: {error}",
                        prepared_lock.path.display()
                    )
                })?
            {
                return Err(format!(
                    "publication lock changed while acquiring it: {}",
                    prepared_lock.path.display()
                ));
            }
            files.push(prepared_lock.file);
            if files.len() == 1 {
                hook.take().expect("lock hook runs once")();
            }
        }
        summary.resolved_directory().verify_current()?;
        report.resolved_directory().verify_current()?;
        Ok(Self { _files: files })
    }
}

pub(super) fn output_lock_path(destination: &Path) -> Result<PathBuf, String> {
    let parent = destination
        .parent()
        .ok_or_else(|| format!("output path has no parent: {}", destination.display()))?;
    let name = destination
        .file_name()
        .ok_or_else(|| format!("output path has no file name: {}", destination.display()))?;
    let label = bounded_file_label(name);
    Ok(parent.join(format!(".key-insights.lock-{label}")))
}

pub(super) fn bounded_file_label(name: &std::ffi::OsStr) -> String {
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

#[cfg(test)]
pub(super) fn open_private_lock_file(path: &Path) -> std::io::Result<File> {
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

#[cfg(all(test, unix))]
pub(super) fn open_file_matches_path(file: &File, path: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let open = file.metadata()?;
    let path = fs::metadata(path)?;
    Ok(open.dev() == path.dev()
        && open.ino() == path.ino()
        && open.nlink() == 1
        && open.mode() & 0o077 == 0)
}

#[cfg(all(test, not(unix)))]
pub(super) fn open_file_matches_path(_file: &File, path: &Path) -> std::io::Result<bool> {
    Ok(fs::metadata(path)?.is_file())
}

pub(super) fn open_private_new_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

pub(super) fn create_private_directory(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}
