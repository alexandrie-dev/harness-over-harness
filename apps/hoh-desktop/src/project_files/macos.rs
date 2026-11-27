//! Open project entries relative to the authorized directory's descriptor on macOS.

use crate::image_asset::ProjectRootIdentity;
use std::{
    ffi::{CStr, CString, OsStr, OsString},
    fs::{File, OpenOptions},
    io,
    mem::MaybeUninit,
    os::{
        fd::{AsRawFd, FromRawFd, IntoRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Component, Path, PathBuf},
    ptr::NonNull,
};

pub(super) fn open_project_directory(
    path: &Path,
    expected: ProjectRootIdentity,
) -> Result<File, String> {
    let open = || -> io::Result<File> {
        if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid project folder",
            ));
        }
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW_ANY)
            .open(path)?;
        let identity = directory.metadata()?;
        if identity.dev() != expected.device
            || identity.ino() != expected.inode
            || identity.nlink() == 0
        {
            return Err(io::Error::other("project folder identity changed"));
        }
        Ok(directory)
    };
    open().map_err(|error| format!("The project folder cannot be opened safely: {error}."))
}

fn entry_stat(parent: &File, name: &CStr) -> io::Result<libc::stat> {
    let mut metadata = MaybeUninit::uninit();
    // SAFETY: the parent descriptor and filename are live. fstatat initializes metadata on success.
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstatat returned success.
    Ok(unsafe { metadata.assume_init() })
}

pub(super) fn open_project_entry(root: &File, relative: &Path) -> io::Result<File> {
    let mut directory = root.try_clone()?;
    if relative == Path::new(".") {
        return Ok(directory);
    }
    let device = directory.metadata()?.dev();
    let mut parts = relative.components().peekable();
    while let Some(part) = parts.next() {
        let Component::Normal(name) = part else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid project entry",
            ));
        };
        let name = CString::new(name.as_bytes())?;
        let expected = entry_stat(&directory, &name)?;
        let kind = expected.st_mode & libc::S_IFMT;
        if kind == libc::S_IFLNK {
            return Err(io::Error::from_raw_os_error(libc::ELOOP));
        }
        if expected.st_dev as u64 != device {
            return Err(io::Error::from_raw_os_error(libc::EXDEV));
        }
        let mut flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
        if kind == libc::S_IFDIR {
            flags |= libc::O_DIRECTORY;
        } else if kind != libc::S_IFREG || parts.peek().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a project file or folder",
            ));
        }
        // SAFETY: openat has a live directory descriptor and one NUL-terminated component.
        let descriptor = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a new descriptor and no other owner exists.
        let child = unsafe { File::from_raw_fd(descriptor) };
        let observed = child.metadata()?;
        if observed.dev() != device
            || observed.ino() != expected.st_ino
            || (observed.mode() as libc::mode_t & libc::S_IFMT) != kind
        {
            return Err(io::Error::other("project entry changed during open"));
        }
        directory = child;
    }
    Ok(directory)
}

pub(super) fn descriptor_path(handle: &File, kind: &str) -> Result<PathBuf, String> {
    let mut buffer = [0_u8; libc::MAXPATHLEN as usize];
    // SAFETY: F_GETPATH writes into the live MAXPATHLEN buffer supplied for the open descriptor.
    if unsafe {
        libc::fcntl(
            handle.as_raw_fd(),
            libc::F_GETPATH,
            buffer.as_mut_ptr().cast::<libc::c_char>(),
        )
    } == -1
    {
        return Err(format!("The {kind} path could not be resolved."));
    }
    let bytes = CStr::from_bytes_until_nul(&buffer)
        .map_err(|_| format!("The {kind} path could not be resolved."))?;
    Ok(PathBuf::from(OsStr::from_bytes(bytes.to_bytes())))
}

struct Directory(NonNull<libc::DIR>);

impl Drop for Directory {
    fn drop(&mut self) {
        // SAFETY: fdopendir handed this stream and its descriptor to the owner once.
        unsafe { libc::closedir(self.0.as_ptr()) };
    }
}

pub(super) fn visit_directory(
    directory: &File,
    limit: usize,
    mut visit: impl FnMut(OsString, &'static str),
) -> io::Result<bool> {
    let duplicate = directory.try_clone()?;
    // SAFETY: the descriptor remains ours on failure and passes to the directory stream on success.
    let handle = unsafe { libc::fdopendir(duplicate.as_raw_fd()) };
    let handle = NonNull::new(handle).ok_or_else(io::Error::last_os_error)?;
    let _ = duplicate.into_raw_fd();
    let stream = Directory(handle);
    let mut count = 0;
    loop {
        // SAFETY: errno is thread-local, and readdir returns data owned by this live stream.
        let entry = unsafe {
            *libc::__error() = 0;
            libc::readdir(stream.0.as_ptr())
        };
        if entry.is_null() {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(0) {
                Ok(false)
            } else {
                Err(error)
            };
        }
        // SAFETY: a non-null readdir result has a NUL-terminated name and lives until the next read.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        if count == limit {
            return Ok(true);
        }
        count += 1;
        let Ok(metadata) = entry_stat(directory, name) else {
            continue;
        };
        let kind = match metadata.st_mode & libc::S_IFMT {
            libc::S_IFDIR => "directory",
            libc::S_IFREG => "file",
            _ => continue,
        };
        visit(OsStr::from_bytes(name.to_bytes()).to_owned(), kind);
    }
}
