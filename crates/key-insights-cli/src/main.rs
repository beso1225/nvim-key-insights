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

struct ResolvedDirectory {
    path: PathBuf,
    file: File,
}

impl ResolvedDirectory {
    #[cfg(unix)]
    fn open(path: &Path) -> Result<Self, String> {
        use std::os::unix::fs::OpenOptionsExt;

        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let file = options.open(path).map_err(|error| {
            format!(
                "failed to open output directory {}: {error}",
                path.display()
            )
        })?;
        let directory = Self {
            path: path.to_owned(),
            file,
        };
        directory.verify_current()?;
        Ok(directory)
    }

    #[cfg(not(unix))]
    fn open(path: &Path) -> Result<Self, String> {
        Err(format!(
            "secure output directory handles are unavailable for {} on this platform",
            path.display()
        ))
    }

    #[cfg(unix)]
    fn verify_current(&self) -> Result<(), String> {
        use std::os::unix::fs::MetadataExt;

        let open = self.file.metadata().map_err(|error| {
            format!(
                "failed to inspect output directory {}: {error}",
                self.path.display()
            )
        })?;
        let current = fs::metadata(&self.path).map_err(|error| {
            format!(
                "output directory changed after resolution {}: {error}",
                self.path.display()
            )
        })?;
        if !current.is_dir() || open.dev() != current.dev() || open.ino() != current.ino() {
            return Err(format!(
                "output directory changed after resolution: {}",
                self.path.display()
            ));
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn verify_current(&self) -> Result<(), String> {
        Err(format!(
            "secure output directory handles are unavailable for {} on this platform",
            self.path.display()
        ))
    }

    #[cfg(unix)]
    fn open_private_new_file(&self, name: &std::ffi::OsStr) -> std::io::Result<File> {
        use std::{
            ffi::CString,
            os::{
                fd::{AsRawFd, FromRawFd},
                unix::ffi::OsStrExt,
            },
        };

        let name = CString::new(name.as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let descriptor = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if descriptor < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(descriptor) })
        }
    }

    #[cfg(not(unix))]
    fn open_private_new_file(&self, _name: &std::ffi::OsStr) -> std::io::Result<File> {
        Err(unsupported_directory_handle_operation())
    }

    #[cfg(unix)]
    fn open_private_lock_file(&self, name: &std::ffi::OsStr) -> std::io::Result<File> {
        use std::{
            ffi::CString,
            os::{
                fd::{AsRawFd, FromRawFd},
                unix::ffi::OsStrExt,
            },
        };

        if self
            .child_metadata(name)?
            .is_some_and(|metadata| !metadata.is_regular_file())
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "lock path is not a regular file",
            ));
        }
        let name = CString::new(name.as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let descriptor = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if descriptor < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(descriptor) })
        }
    }

    #[cfg(not(unix))]
    fn open_private_lock_file(&self, _name: &std::ffi::OsStr) -> std::io::Result<File> {
        Err(unsupported_directory_handle_operation())
    }

    #[cfg(unix)]
    fn open_read_file(&self, name: &std::ffi::OsStr) -> std::io::Result<Option<File>> {
        use std::{
            ffi::CString,
            os::{
                fd::{AsRawFd, FromRawFd},
                unix::ffi::OsStrExt,
                unix::fs::MetadataExt,
            },
        };

        let Some(metadata) = self.child_metadata(name)? else {
            return Ok(None);
        };
        if !metadata.is_regular_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "recovery artifact is not a regular file",
            ));
        }
        let name = CString::new(name.as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let descriptor = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if descriptor < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error)
            }
        } else {
            let file = unsafe { File::from_raw_fd(descriptor) };
            let open = file.metadata()?;
            if open.dev() != metadata.device_u64()
                || open.ino() != metadata.inode_u64()
                || open.mode() & 0o077 != 0
            {
                return Err(std::io::Error::other(
                    "recovery artifact changed while opening it",
                ));
            }
            Ok(Some(file))
        }
    }

    #[cfg(not(unix))]
    fn open_read_file(&self, _name: &std::ffi::OsStr) -> std::io::Result<Option<File>> {
        Err(unsupported_directory_handle_operation())
    }

    #[cfg(unix)]
    fn open_file_matches_child(
        &self,
        file: &File,
        name: &std::ffi::OsStr,
    ) -> std::io::Result<bool> {
        use std::os::unix::fs::MetadataExt;

        let open = file.metadata()?;
        let Some(child) = self.child_metadata(name)? else {
            return Ok(false);
        };
        Ok(open.dev() == child.device_u64()
            && open.ino() == child.inode_u64()
            && child.links == 1
            && child.mode & 0o077 == 0)
    }

    #[cfg(unix)]
    fn open_file_has_child_identity(
        &self,
        file: &File,
        name: &std::ffi::OsStr,
    ) -> std::io::Result<bool> {
        use std::os::unix::fs::MetadataExt;

        let open = file.metadata()?;
        let Some(child) = self.child_metadata(name)? else {
            return Ok(false);
        };
        Ok(open.dev() == child.device_u64() && open.ino() == child.inode_u64())
    }

    #[cfg(not(unix))]
    fn open_file_has_child_identity(
        &self,
        _file: &File,
        _name: &std::ffi::OsStr,
    ) -> std::io::Result<bool> {
        Err(unsupported_directory_handle_operation())
    }

    #[cfg(not(unix))]
    fn open_file_matches_child(
        &self,
        _file: &File,
        _name: &std::ffi::OsStr,
    ) -> std::io::Result<bool> {
        Err(unsupported_directory_handle_operation())
    }

    #[cfg(unix)]
    fn rename_child(
        &self,
        source: &std::ffi::OsStr,
        destination: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        use std::{
            ffi::CString,
            os::{fd::AsRawFd, unix::ffi::OsStrExt},
        };

        let source = CString::new(source.as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let destination = CString::new(destination.as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let result = unsafe {
            libc::renameat(
                self.file.as_raw_fd(),
                source.as_ptr(),
                self.file.as_raw_fd(),
                destination.as_ptr(),
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    #[cfg(not(unix))]
    fn rename_child(
        &self,
        _source: &std::ffi::OsStr,
        _destination: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        Err(unsupported_directory_handle_operation())
    }

    #[cfg(unix)]
    fn remove_child(&self, name: &std::ffi::OsStr) -> std::io::Result<()> {
        use std::{
            ffi::CString,
            os::{fd::AsRawFd, unix::ffi::OsStrExt},
        };

        let name = CString::new(name.as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let result = unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0) };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    #[cfg(not(unix))]
    fn remove_child(&self, _name: &std::ffi::OsStr) -> std::io::Result<()> {
        Err(unsupported_directory_handle_operation())
    }

    #[cfg(unix)]
    fn link_child(
        &self,
        source: &std::ffi::OsStr,
        destination: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        use std::{
            ffi::CString,
            os::{fd::AsRawFd, unix::ffi::OsStrExt},
        };

        let source = CString::new(source.as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let destination = CString::new(destination.as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let result = unsafe {
            libc::linkat(
                self.file.as_raw_fd(),
                source.as_ptr(),
                self.file.as_raw_fd(),
                destination.as_ptr(),
                0,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    #[cfg(not(unix))]
    fn link_child(
        &self,
        _source: &std::ffi::OsStr,
        _destination: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        Err(unsupported_directory_handle_operation())
    }

    #[cfg(unix)]
    fn child_metadata(&self, name: &std::ffi::OsStr) -> std::io::Result<Option<ChildMetadata>> {
        use std::{
            ffi::CString,
            mem::MaybeUninit,
            os::{fd::AsRawFd, unix::ffi::OsStrExt},
        };

        let name = CString::new(name.as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let mut metadata = MaybeUninit::<libc::stat>::uninit();
        let result = unsafe {
            libc::fstatat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result == 0 {
            let metadata = unsafe { metadata.assume_init() };
            Ok(Some(ChildMetadata {
                device: metadata.st_dev,
                inode: metadata.st_ino,
                mode: metadata.st_mode,
                links: metadata.st_nlink,
            }))
        } else {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }

    #[cfg(not(unix))]
    fn child_metadata(&self, _name: &std::ffi::OsStr) -> std::io::Result<Option<ChildMetadata>> {
        Err(unsupported_directory_handle_operation())
    }

    fn sync(&self) -> std::io::Result<()> {
        self.file.sync_all()
    }
}

#[cfg(not(unix))]
fn unsupported_directory_handle_operation() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "secure directory-handle operation is unavailable on this platform",
    )
}

#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct ChildMetadata {
    device: libc::dev_t,
    inode: libc::ino_t,
    mode: libc::mode_t,
    links: libc::nlink_t,
}

#[cfg(unix)]
impl ChildMetadata {
    fn is_regular_file(self) -> bool {
        self.mode & libc::S_IFMT == libc::S_IFREG
    }

    #[allow(clippy::unnecessary_cast)]
    fn device_u64(self) -> u64 {
        self.device as u64
    }

    #[allow(clippy::unnecessary_cast)]
    fn inode_u64(self) -> u64 {
        self.inode as u64
    }
}

#[cfg(not(unix))]
#[derive(Clone, Copy, PartialEq, Eq)]
struct ChildMetadata;

#[cfg(not(unix))]
impl ChildMetadata {
    fn is_regular_file(self) -> bool {
        false
    }

    fn device_u64(self) -> u64 {
        0
    }

    fn inode_u64(self) -> u64 {
        0
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
    temporary_name: Option<std::ffi::OsString>,
    destination: PathBuf,
    destination_name: std::ffi::OsString,
    directory: Arc<ResolvedDirectory>,
}

impl StagedOutput {
    fn create<D: OutputDestination + ?Sized>(
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

    fn publish(mut self) -> Result<(), String> {
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
    fn temporary_path(&self) -> PathBuf {
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

trait OutputDestination {
    fn resolve_destination(&self) -> Result<(PathBuf, Arc<ResolvedDirectory>), String>;
}

trait AnchoredOutput {
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

struct OutputBackup {
    destination: PathBuf,
    backup_path: Option<PathBuf>,
    anchored: Option<AnchoredBackup>,
}

struct AnchoredBackup {
    directory: Arc<ResolvedDirectory>,
    destination_name: std::ffi::OsString,
    backup_name: Option<std::ffi::OsString>,
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
                    anchored: None,
                })
            }
            _ => Self::capture_at(destination, unused_sibling_name(destination, "backup")?),
        }
    }

    #[cfg(test)]
    fn capture_at(destination: &Path, backup_path: PathBuf) -> Result<Self, String> {
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

    fn restore(&mut self) -> Result<(), String> {
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

    fn discard(&mut self) -> Result<(), String> {
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

    fn capture_anchored(output: &StagedOutput, backup_path: PathBuf) -> Result<Self, String> {
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
            Some(_) => {
                output
                    .directory
                    .link_child(&output.destination_name, &backup_name)
                    .map_err(|error| {
                        format!(
                            "failed to link existing output {} into a backup: {error}",
                            output.destination.display()
                        )
                    })?;
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

    fn restore_anchored(&mut self) -> Result<(), String> {
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

    fn discard_anchored(&mut self) -> Result<(), String> {
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

fn unique_child_name(name: &std::ffi::OsStr, label: &str) -> std::ffi::OsString {
    let identifier = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        ".key-insights.{}.{label}-{}-{identifier}",
        bounded_file_label(name),
        std::process::id()
    )
    .into()
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
    directories: Option<[Arc<ResolvedDirectory>; 2]>,
}

#[cfg(test)]
fn recover_outputs(summary: &Path, report: &Path) -> Result<(), String> {
    let _locks = OutputLocks::acquire(summary, report)?;
    recover_destination(summary)?;
    recover_destination(report)?;
    let paths = PairRecoveryPaths::new(summary, report)?;
    recover_pair(summary, report, &paths)
}

fn recover_outputs_anchored(
    summary: &ResolvedOutputPath,
    report: &ResolvedOutputPath,
) -> Result<(), String> {
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
    report.directory.verify_current()
}

fn read_recovery_index_anchored(
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

fn read_recovery_marker_anchored(
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

fn open_recorded_directory(
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

fn open_optional_recorded_directory(
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

fn anchored_child_same_file(
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

fn anchored_recovery_backup(
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
        }),
    })
}

fn recover_destination_anchored(
    output: &ResolvedOutputPath,
    known_outputs: [&ResolvedOutputPath; 2],
) -> Result<(), String> {
    recover_destination_anchored_with_hook(output, known_outputs, || {})
}

fn recover_destination_anchored_with_hook<F>(
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

fn recover_pair_anchored(
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
            directories: None,
        })
    }

    fn begin_anchored(summary: &StagedOutput, report: &StagedOutput) -> Result<Self, String> {
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

    fn rollback(mut self, error: String) -> String {
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

    fn commit(mut self) -> Result<(), String> {
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

    fn rollback_anchored(&mut self, error: String) -> String {
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

    fn commit_anchored(&mut self) -> Result<(), String> {
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

fn create_destination_recovery_indexes_anchored(
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

fn publish_private_sidecar_anchored(
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

fn transition_child_marker(
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
            if source.is_none() || source != destination {
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
        if destination_metadata != Some(source_metadata) {
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

fn remove_anchored_recovery_indexes(
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

fn remove_anchored_file(
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

#[cfg(test)]
fn write_destination_recovery_index(
    path: &Path,
    index: &DestinationRecoveryIndex,
) -> Result<(), String> {
    let contents = serde_json::to_vec(index)
        .map_err(|error| format!("failed to encode destination recovery index: {error}"))?;
    publish_private_sidecar(path, &contents)
        .map_err(|error| format!("failed to publish destination recovery index: {error}"))
}

#[cfg(test)]
fn recover_destination(destination: &Path) -> Result<(), String> {
    recover_destination_with_hook(destination, || {})
}

#[cfg(test)]
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

#[cfg(all(test, unix))]
fn open_recovery_index_matches_path(file: &File, path: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let open = file.metadata()?;
    let path = fs::metadata(path)?;
    Ok(open.dev() == path.dev()
        && open.ino() == path.ino()
        && open.nlink() == 1
        && open.mode() & 0o077 == 0)
}

#[cfg(all(test, not(unix)))]
fn open_recovery_index_matches_path(_file: &File, path: &Path) -> std::io::Result<bool> {
    Ok(fs::metadata(path)?.is_file())
}

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
fn publish_private_sidecar(path: &Path, contents: &[u8]) -> Result<(), String> {
    publish_private_sidecar_with(path, contents, |file, contents| {
        file.write_all(contents)?;
        file.sync_all()
    })
}

#[cfg(test)]
fn publish_private_sidecar_with<F>(path: &Path, contents: &[u8], persist: F) -> Result<(), String>
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
fn create_recovery_marker(path: &Path, previous_outputs: [bool; 2]) -> Result<(), String> {
    let contents = [
        if previous_outputs[0] { b'1' } else { b'0' },
        if previous_outputs[1] { b'1' } else { b'0' },
        b'\n',
    ];
    publish_private_sidecar(path, &contents)
        .map_err(|error| format!("failed to publish recovery marker: {error}"))
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

#[cfg(test)]
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

#[cfg(all(test, unix))]
fn open_recovery_marker_matches_path(file: &File, path: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let open = file.metadata()?;
    let path = fs::metadata(path)?;
    Ok(open.dev() == path.dev()
        && open.ino() == path.ino()
        && matches!(open.nlink(), 1 | 2)
        && open.mode() & 0o077 == 0)
}

#[cfg(all(test, not(unix)))]
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

#[cfg(all(test, target_os = "linux"))]
fn rename_without_replacement(source: &Path, destination: &Path) -> std::io::Result<()> {
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
fn rename_without_replacement(source: &Path, destination: &Path) -> std::io::Result<()> {
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
fn rename_without_replacement(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(all(test, not(any(target_os = "linux", target_os = "macos", windows))))]
fn rename_without_replacement(_source: &Path, _destination: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic no-replace rename is unavailable on this platform",
    ))
}

struct OutputLocks {
    _files: Vec<File>,
}

impl OutputLocks {
    #[cfg(test)]
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

    fn acquire_anchored<S: AnchoredOutput, R: AnchoredOutput>(
        summary: &S,
        report: &R,
    ) -> Result<Self, String> {
        Self::acquire_anchored_with_hook(summary, report, || {})
    }

    fn acquire_anchored_with_hook<S: AnchoredOutput, R: AnchoredOutput, F>(
        summary: &S,
        report: &R,
        after_first_lock: F,
    ) -> Result<Self, String>
    where
        F: FnOnce(),
    {
        struct Candidate<'a> {
            path: PathBuf,
            name: std::ffi::OsString,
            directory: &'a ResolvedDirectory,
        }

        let mut candidates = vec![
            Candidate {
                path: output_lock_path(summary.destination_path())?,
                name: output_lock_path(summary.destination_path())?
                    .file_name()
                    .expect("summary lock name")
                    .to_owned(),
                directory: summary.resolved_directory(),
            },
            Candidate {
                path: output_lock_path(report.destination_path())?,
                name: output_lock_path(report.destination_path())?
                    .file_name()
                    .expect("report lock name")
                    .to_owned(),
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

        let mut files = Vec::with_capacity(candidates.len());
        let mut hook = Some(after_first_lock);
        for candidate in candidates {
            let file = candidate
                .directory
                .open_private_lock_file(&candidate.name)
                .map_err(|error| {
                    format!(
                        "failed to open publication lock {}: {error}",
                        candidate.path.display()
                    )
                })?;
            file.lock().map_err(|error| {
                format!(
                    "failed to acquire publication lock {}: {error}",
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
                    "publication lock changed while acquiring it: {}",
                    candidate.path.display()
                ));
            }
            files.push(file);
            if files.len() == 1 {
                hook.take().expect("lock hook runs once")();
            }
        }
        summary.resolved_directory().verify_current()?;
        report.resolved_directory().verify_current()?;
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

#[cfg(test)]
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

#[cfg(all(test, unix))]
fn open_file_matches_path(file: &File, path: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let open = file.metadata()?;
    let path = fs::metadata(path)?;
    Ok(open.dev() == path.dev()
        && open.ino() == path.ino()
        && open.nlink() == 1
        && open.mode() & 0o077 == 0)
}

#[cfg(all(test, not(unix)))]
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
    fn staging_rejects_a_swapped_output_ancestor() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-ancestor-swap-{}-{unique}",
            std::process::id()
        ));
        let output_directory = directory.join("outputs");
        let moved_directory = directory.join("moved-outputs");
        let attacker_directory = directory.join("attacker");
        fs::create_dir_all(&output_directory).expect("create output directory");
        fs::create_dir(&attacker_directory).expect("create attacker directory");
        let input = directory.join("input.jsonl");
        let summary = output_directory.join("summary.json");
        let report = output_directory.join("report.md");
        fs::write(&input, "raw input\n").expect("write input");
        let paths = resolve_paths(&input, &summary, &report).expect("resolve safe paths");

        fs::rename(&output_directory, &moved_directory).expect("move resolved output directory");
        symlink(&attacker_directory, &output_directory).expect("replace ancestor with symlink");

        let error = match StagedOutput::create(&paths.summary, b"summary\n") {
            Ok(_) => panic!("swapped ancestor must reject staging"),
            Err(error) => error,
        };
        assert!(
            error.contains("output directory changed"),
            "unexpected staging error: {error}"
        );
        assert!(
            !attacker_directory.join("summary.json").exists(),
            "staging must not follow the replacement ancestor"
        );
        assert!(
            !moved_directory.join("summary.json").exists(),
            "rejected staging must not create output in the moved directory"
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn startup_recovery_rejects_a_swapped_output_ancestor() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-recovery-ancestor-swap-{}-{unique}",
            std::process::id()
        ));
        let output_directory = directory.join("outputs");
        let moved_directory = directory.join("moved-outputs");
        let attacker_directory = directory.join("attacker");
        fs::create_dir_all(&output_directory).expect("create output directory");
        fs::create_dir(&attacker_directory).expect("create attacker directory");
        let input = directory.join("input.jsonl");
        let summary = output_directory.join("summary.json");
        let report = output_directory.join("report.md");
        fs::write(&input, "raw input\n").expect("write input");
        let paths = resolve_paths(&input, &summary, &report).expect("resolve safe paths");

        fs::rename(&output_directory, &moved_directory).expect("move resolved output directory");
        symlink(&attacker_directory, &output_directory).expect("replace ancestor with symlink");

        let error = super::recover_outputs_anchored(&paths.summary, &paths.report)
            .expect_err("swapped ancestor must reject startup recovery");
        assert!(
            error.contains("output directory changed"),
            "unexpected recovery error: {error}"
        );
        assert_eq!(
            fs::read_dir(&attacker_directory)
                .expect("read attacker directory")
                .count(),
            0,
            "startup recovery must not follow the replacement ancestor"
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn startup_recovery_does_not_follow_an_ancestor_swapped_after_index_read() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-mid-recovery-ancestor-swap-{}-{unique}",
            std::process::id()
        ));
        let output_directory = directory.join("outputs");
        let moved_directory = directory.join("moved-outputs");
        let attacker_directory = directory.join("attacker");
        fs::create_dir_all(&output_directory).expect("create output directory");
        fs::create_dir(&attacker_directory).expect("create attacker directory");
        let input = directory.join("input.jsonl");
        let summary = output_directory.join("summary.json");
        let report = output_directory.join("report.md");
        fs::write(&input, "raw input\n").expect("write input");
        fs::write(&summary, "old summary\n").expect("write old summary");
        fs::write(&report, "old report\n").expect("write old report");
        let paths = resolve_paths(&input, &summary, &report).expect("resolve recovery paths");
        let interrupted = catch_unwind(AssertUnwindSafe(|| {
            publish_pair_with_hook(
                StagedOutput::create(&paths.summary, b"new summary\n").expect("stage summary"),
                StagedOutput::create(&paths.report, b"new report\n").expect("stage report"),
                || panic!("interrupt publication after summary"),
            )
        }));
        assert!(interrupted.is_err(), "publication must be interrupted");
        let outputs = [&paths.summary, &paths.report];

        let error = super::recover_destination_anchored_with_hook(&paths.summary, outputs, || {
            fs::rename(&output_directory, &moved_directory)
                .expect("move resolved output directory");
            symlink(&attacker_directory, &output_directory).expect("replace ancestor with symlink");
        })
        .expect_err("ancestor swap must fail closed after anchored recovery");

        assert!(
            error.contains("output directory changed"),
            "unexpected recovery error: {error}"
        );
        assert_eq!(
            fs::read_to_string(moved_directory.join("summary.json"))
                .expect("read restored summary"),
            "old summary\n"
        );
        assert_eq!(
            fs::read_to_string(moved_directory.join("report.md")).expect("read old report"),
            "old report\n"
        );
        assert_eq!(
            fs::read_dir(&attacker_directory)
                .expect("read attacker directory")
                .count(),
            0,
            "startup recovery must not follow the replacement ancestor"
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn publication_rejects_an_ancestor_swapped_after_staging() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-publication-ancestor-swap-{}-{unique}",
            std::process::id()
        ));
        let output_directory = directory.join("outputs");
        let moved_directory = directory.join("moved-outputs");
        let attacker_directory = directory.join("attacker");
        fs::create_dir_all(&output_directory).expect("create output directory");
        fs::create_dir(&attacker_directory).expect("create attacker directory");
        let input = directory.join("input.jsonl");
        let summary = output_directory.join("summary.json");
        let report = output_directory.join("report.md");
        fs::write(&input, "raw input\n").expect("write input");
        let paths = resolve_paths(&input, &summary, &report).expect("resolve safe paths");
        let summary_output =
            StagedOutput::create(&paths.summary, b"summary\n").expect("stage summary");
        let report_output = StagedOutput::create(&paths.report, b"report\n").expect("stage report");

        fs::rename(&output_directory, &moved_directory).expect("move resolved output directory");
        symlink(&attacker_directory, &output_directory).expect("replace ancestor with symlink");

        let error = publish_pair(summary_output, report_output)
            .expect_err("swapped ancestor must reject publication");
        assert!(
            error.contains("output directory changed"),
            "unexpected publication error: {error}"
        );
        assert_eq!(
            fs::read_dir(&attacker_directory)
                .expect("read attacker directory")
                .count(),
            0,
            "publication must not create attacker-controlled artifacts"
        );
        assert!(
            !moved_directory.join("summary.json").exists()
                && !moved_directory.join("report.md").exists(),
            "rejected publication must not create public outputs in the moved directory"
        );
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn lock_acquisition_does_not_follow_an_ancestor_swap() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-lock-ancestor-swap-{}-{unique}",
            std::process::id()
        ));
        let output_directory = directory.join("outputs");
        let moved_directory = directory.join("moved-outputs");
        let attacker_directory = directory.join("attacker");
        fs::create_dir_all(&output_directory).expect("create output directory");
        fs::create_dir(&attacker_directory).expect("create attacker directory");
        let summary = output_directory.join("summary.json");
        let report = output_directory.join("report.md");
        let summary_output = StagedOutput::create(&summary, b"summary\n").expect("stage summary");
        let report_output = StagedOutput::create(&report, b"report\n").expect("stage report");

        let error =
            match OutputLocks::acquire_anchored_with_hook(&summary_output, &report_output, || {
                fs::rename(&output_directory, &moved_directory)
                    .expect("move resolved output directory");
                symlink(&attacker_directory, &output_directory)
                    .expect("replace ancestor with symlink");
            }) {
                Ok(_) => panic!("ancestor swap must reject lock acquisition"),
                Err(error) => error,
            };

        assert!(
            error.contains("output directory changed"),
            "unexpected lock error: {error}"
        );
        assert_eq!(
            fs::read_dir(&attacker_directory)
                .expect("read attacker directory")
                .count(),
            0,
            "lock acquisition must not follow the replacement ancestor"
        );
        drop(summary_output);
        drop(report_output);
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn publication_rolls_back_when_an_ancestor_is_swapped_after_the_first_output() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-mid-publication-ancestor-swap-{}-{unique}",
            std::process::id()
        ));
        let output_directory = directory.join("outputs");
        let moved_directory = directory.join("moved-outputs");
        let attacker_directory = directory.join("attacker");
        fs::create_dir_all(&output_directory).expect("create output directory");
        fs::create_dir(&attacker_directory).expect("create attacker directory");
        let summary = output_directory.join("summary.json");
        let report = output_directory.join("report.md");
        fs::write(&summary, "old summary\n").expect("write old summary");
        fs::write(&report, "old report\n").expect("write old report");
        let summary_output =
            StagedOutput::create(&summary, b"new summary\n").expect("stage summary");
        let report_output = StagedOutput::create(&report, b"new report\n").expect("stage report");

        let error = publish_pair_with_hook(summary_output, report_output, || {
            fs::rename(&output_directory, &moved_directory)
                .expect("move resolved output directory");
            symlink(&attacker_directory, &output_directory).expect("replace ancestor with symlink");
        })
        .expect_err("swapped ancestor must fail publication and roll back");

        assert!(
            error.contains("output directory changed"),
            "unexpected publication error: {error}"
        );
        assert_eq!(
            fs::read_to_string(moved_directory.join("summary.json"))
                .expect("read restored summary"),
            "old summary\n"
        );
        assert_eq!(
            fs::read_to_string(moved_directory.join("report.md")).expect("read restored report"),
            "old report\n"
        );
        assert_eq!(
            fs::read_dir(&attacker_directory)
                .expect("read attacker directory")
                .count(),
            0,
            "rollback must not follow the replacement ancestor"
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
        fs::remove_file(report_output.temporary_path()).expect("force second publication failure");

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
        fs::remove_file(retry_summary.temporary_path()).expect("force retry publication failure");
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
        fs::remove_file(retry_summary.temporary_path()).expect("force retry publication failure");
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
    fn partial_sidecar_write_is_never_published_and_retry_succeeds() {
        use std::io::Write;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-sidecar-write-failure-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create test directory");
        let sidecar = directory.join("recovery-index");

        super::publish_private_sidecar_with(&sidecar, b"complete", |file, _contents| {
            file.write_all(b"partial")?;
            Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "injected sidecar write failure",
            ))
        })
        .expect_err("partial sidecar write must fail");
        assert!(
            !sidecar.exists(),
            "partial final sidecar must not be visible"
        );

        super::publish_private_sidecar_with(&sidecar, b"complete", |file, contents| {
            file.write_all(contents)?;
            file.sync_all()
        })
        .expect("retry sidecar publication");
        assert_eq!(fs::read(&sidecar).expect("read sidecar"), b"complete");
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn sidecar_publication_never_replaces_an_existing_entry() {
        use std::io::Write;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-occupied-sidecar-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create test directory");
        let sidecar = directory.join("recovery-marker");
        fs::write(&sidecar, "unrelated\n").expect("write occupied sidecar");

        super::publish_private_sidecar_with(&sidecar, b"ours", |file, contents| {
            file.write_all(contents)?;
            file.sync_all()
        })
        .expect_err("occupied sidecar must reject publication");

        assert_eq!(
            fs::read_to_string(&sidecar).expect("read occupied sidecar"),
            "unrelated\n"
        );
        let temporary_files = fs::read_dir(&directory)
            .expect("read test directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(
            temporary_files, 0,
            "failed publication cleans its temporary file"
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
