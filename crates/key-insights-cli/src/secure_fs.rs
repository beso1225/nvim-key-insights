use super::*;

pub(super) struct ResolvedDirectory {
    pub(super) path: PathBuf,
    pub(super) file: File,
}

impl ResolvedDirectory {
    #[cfg(unix)]
    pub(super) fn child_names(
        &self,
        maximum_entries: usize,
    ) -> std::io::Result<Vec<std::ffi::OsString>> {
        use std::{
            ffi::{CStr, OsString},
            os::{fd::AsRawFd, unix::ffi::OsStringExt},
        };

        let descriptor = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                c".".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let stream = unsafe { libc::fdopendir(descriptor) };
        if stream.is_null() {
            let error = std::io::Error::last_os_error();
            unsafe { libc::close(descriptor) };
            return Err(error);
        }

        let result = (|| {
            let mut names = Vec::new();
            loop {
                clear_readdir_error();
                let entry = unsafe { libc::readdir(stream) };
                if entry.is_null() {
                    if let Some(error) = readdir_error() {
                        return Err(error);
                    }
                    break;
                }
                let bytes = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
                if bytes == b"." || bytes == b".." {
                    continue;
                }
                if names.len() >= maximum_entries {
                    return Err(std::io::Error::other(format!(
                        "directory entry count exceeds the limit of {maximum_entries}"
                    )));
                }
                names.push(OsString::from_vec(bytes.to_vec()));
            }
            Ok(names)
        })();
        let close_result = unsafe { libc::closedir(stream) };
        if close_result != 0 && result.is_ok() {
            return Err(std::io::Error::last_os_error());
        }
        result
    }

    #[cfg(not(unix))]
    pub(super) fn child_names(
        &self,
        _maximum_entries: usize,
    ) -> std::io::Result<Vec<std::ffi::OsString>> {
        Err(unsupported_directory_handle_operation())
    }

    #[cfg(unix)]
    pub(super) fn open(path: &Path) -> Result<Self, String> {
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
    pub(super) fn open(path: &Path) -> Result<Self, String> {
        Err(format!(
            "secure output directory handles are unavailable for {} on this platform",
            path.display()
        ))
    }

    #[cfg(unix)]
    pub(super) fn verify_current(&self) -> Result<(), String> {
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
    pub(super) fn verify_current(&self) -> Result<(), String> {
        Err(format!(
            "secure output directory handles are unavailable for {} on this platform",
            self.path.display()
        ))
    }

    #[cfg(unix)]
    pub(super) fn open_private_new_file(&self, name: &std::ffi::OsStr) -> std::io::Result<File> {
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
    pub(super) fn open_private_new_file(&self, _name: &std::ffi::OsStr) -> std::io::Result<File> {
        Err(unsupported_directory_handle_operation())
    }

    #[cfg(unix)]
    pub(super) fn open_private_lock_file(&self, name: &std::ffi::OsStr) -> std::io::Result<File> {
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
                libc::O_RDWR
                    | libc::O_CREAT
                    | libc::O_CLOEXEC
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK,
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
    pub(super) fn open_private_lock_file(&self, _name: &std::ffi::OsStr) -> std::io::Result<File> {
        Err(unsupported_directory_handle_operation())
    }

    #[cfg(unix)]
    pub(super) fn open_read_file(&self, name: &std::ffi::OsStr) -> std::io::Result<Option<File>> {
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
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
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
                || open.mode() & 0o7777 != 0o600
                || open.uid() != unsafe { libc::geteuid() }
            {
                return Err(std::io::Error::other(
                    "recovery artifact changed while opening it",
                ));
            }
            Ok(Some(file))
        }
    }

    #[cfg(not(unix))]
    pub(super) fn open_read_file(&self, _name: &std::ffi::OsStr) -> std::io::Result<Option<File>> {
        Err(unsupported_directory_handle_operation())
    }

    #[cfg(unix)]
    pub(super) fn open_file_matches_child(
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
            && open.is_file()
            && open.nlink() == 1
            && open.mode() & 0o7777 == 0o600
            && open.uid() == unsafe { libc::geteuid() }
            && child.is_private_file_owned_by_current_user())
    }

    #[cfg(unix)]
    pub(super) fn open_file_has_child_identity(
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
    pub(super) fn open_file_has_child_identity(
        &self,
        _file: &File,
        _name: &std::ffi::OsStr,
    ) -> std::io::Result<bool> {
        Err(unsupported_directory_handle_operation())
    }

    #[cfg(not(unix))]
    pub(super) fn open_file_matches_child(
        &self,
        _file: &File,
        _name: &std::ffi::OsStr,
    ) -> std::io::Result<bool> {
        Err(unsupported_directory_handle_operation())
    }

    #[cfg(unix)]
    pub(super) fn rename_child(
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
    pub(super) fn rename_child(
        &self,
        _source: &std::ffi::OsStr,
        _destination: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        Err(unsupported_directory_handle_operation())
    }

    #[cfg(unix)]
    pub(super) fn remove_child(&self, name: &std::ffi::OsStr) -> std::io::Result<()> {
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
    pub(super) fn remove_child(&self, _name: &std::ffi::OsStr) -> std::io::Result<()> {
        Err(unsupported_directory_handle_operation())
    }

    #[cfg(unix)]
    pub(super) fn link_child(
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
    pub(super) fn link_child(
        &self,
        _source: &std::ffi::OsStr,
        _destination: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        Err(unsupported_directory_handle_operation())
    }

    #[cfg(unix)]
    pub(super) fn child_metadata(
        &self,
        name: &std::ffi::OsStr,
    ) -> std::io::Result<Option<ChildMetadata>> {
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
                owner: metadata.st_uid,
                modified_seconds: metadata.st_mtime,
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
    pub(super) fn child_metadata(
        &self,
        _name: &std::ffi::OsStr,
    ) -> std::io::Result<Option<ChildMetadata>> {
        Err(unsupported_directory_handle_operation())
    }

    pub(super) fn sync(&self) -> std::io::Result<()> {
        self.file.sync_all()
    }
}

#[cfg(unix)]
macro_rules! define_readdir_errno {
    ($location:path) => {
        fn clear_readdir_error() {
            unsafe { *$location() = 0 };
        }

        fn readdir_error() -> Option<std::io::Error> {
            let code = unsafe { *$location() };
            (code != 0).then(|| std::io::Error::from_raw_os_error(code))
        }
    };
}

#[cfg(any(target_os = "linux", target_os = "android"))]
define_readdir_errno!(libc::__errno_location);

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
define_readdir_errno!(libc::__error);

#[cfg(all(
    unix,
    not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))
))]
mod unsupported_readdir_errno {
    pub(super) fn clear_readdir_error() {}

    pub(super) fn readdir_error() -> Option<std::io::Error> {
        None
    }
}

#[cfg(all(
    unix,
    not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))
))]
use unsupported_readdir_errno::{clear_readdir_error, readdir_error};

#[cfg(not(unix))]
pub(super) fn unsupported_directory_handle_operation() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "secure directory-handle operation is unavailable on this platform",
    )
}

#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct ChildMetadata {
    device: libc::dev_t,
    inode: libc::ino_t,
    mode: libc::mode_t,
    links: libc::nlink_t,
    owner: libc::uid_t,
    modified_seconds: libc::time_t,
}

#[cfg(unix)]
impl ChildMetadata {
    pub(super) fn is_regular_file(self) -> bool {
        self.mode & libc::S_IFMT == libc::S_IFREG
    }

    pub(super) fn same_identity(self, other: Self) -> bool {
        self.device == other.device && self.inode == other.inode
    }

    pub(super) fn same_regular_file(self, other: Self) -> bool {
        self.is_regular_file() && other.is_regular_file() && self.same_identity(other)
    }

    pub(super) fn is_private_file_owned_by_current_user(self) -> bool {
        self.is_regular_file()
            && self.links == 1
            && self.mode & 0o7777 == 0o600
            && self.owner == unsafe { libc::geteuid() }
    }

    pub(super) fn is_at_least_age(self, now_seconds: u64, age_seconds: u64) -> bool {
        u64::try_from(self.modified_seconds)
            .ok()
            .is_some_and(|modified| {
                modified <= now_seconds && now_seconds - modified >= age_seconds
            })
    }

    #[allow(clippy::unnecessary_cast)]
    pub(super) fn device_u64(self) -> u64 {
        self.device as u64
    }

    #[allow(clippy::unnecessary_cast)]
    pub(super) fn inode_u64(self) -> u64 {
        self.inode as u64
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{ChildMetadata, ResolvedDirectory};

    #[test]
    fn anchored_directory_scans_do_not_share_offsets() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "key-insights-directory-rescan-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create directory");
        fs::write(directory.join("one"), "one").expect("write entry");
        let resolved = ResolvedDirectory::open(&directory).expect("open directory");

        let first = resolved.child_names(10).expect("first scan");
        let second = resolved.child_names(10).expect("second scan");

        assert_eq!(first, second);
        fs::remove_dir_all(directory).expect("remove directory");
    }

    #[test]
    fn matching_inode_is_not_enough_when_the_current_file_type_changed() {
        let regular = ChildMetadata {
            device: 1,
            inode: 2,
            mode: libc::S_IFREG | 0o600,
            links: 1,
            owner: 3,
            modified_seconds: 4,
        };
        let symlink = ChildMetadata {
            mode: libc::S_IFLNK | 0o777,
            ..regular
        };

        assert!(!regular.same_regular_file(symlink));
    }

    #[test]
    fn private_files_reject_special_permission_bits() {
        let special = ChildMetadata {
            device: 1,
            inode: 2,
            mode: libc::S_IFREG | 0o1600,
            links: 1,
            owner: unsafe { libc::geteuid() },
            modified_seconds: 4,
        };

        assert!(!special.is_private_file_owned_by_current_user());
    }
}

#[cfg(not(unix))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct ChildMetadata;

#[cfg(not(unix))]
impl ChildMetadata {
    pub(super) fn is_regular_file(self) -> bool {
        false
    }

    pub(super) fn same_identity(self, _other: Self) -> bool {
        false
    }

    pub(super) fn same_regular_file(self, _other: Self) -> bool {
        false
    }

    pub(super) fn is_private_file_owned_by_current_user(self) -> bool {
        false
    }

    pub(super) fn is_at_least_age(self, _now_seconds: u64, _age_seconds: u64) -> bool {
        false
    }

    pub(super) fn device_u64(self) -> u64 {
        0
    }

    pub(super) fn inode_u64(self) -> u64 {
        0
    }
}
