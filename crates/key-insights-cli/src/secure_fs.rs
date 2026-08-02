use super::*;

pub(super) struct ResolvedDirectory {
    pub(super) path: PathBuf,
    pub(super) file: File,
}

impl ResolvedDirectory {
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
            && child.is_regular_file()
            && child.links == 1
            && child.mode & 0o077 == 0)
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
}

#[cfg(unix)]
impl ChildMetadata {
    pub(super) fn is_regular_file(self) -> bool {
        self.mode & libc::S_IFMT == libc::S_IFREG
    }

    pub(super) fn same_identity(self, other: Self) -> bool {
        self.device == other.device && self.inode == other.inode
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

    pub(super) fn device_u64(self) -> u64 {
        0
    }

    pub(super) fn inode_u64(self) -> u64 {
        0
    }
}
