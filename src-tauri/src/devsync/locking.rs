use std::{
    fs::{File, OpenOptions},
    path::Path,
};

use super::{DevSyncError, paths::ensure_directory};

pub struct ProcessLock {
    file: File,
}

impl ProcessLock {
    pub fn try_acquire(path: &Path) -> Result<Option<Self>, DevSyncError> {
        if let Some(parent) = path.parent() {
            ensure_directory(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| {
                DevSyncError::new(
                    "lock_error",
                    format!("Unable to open lock {}: {error}", path.display()),
                )
            })?;

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;

            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if result == 0 {
                return Ok(Some(Self { file }));
            }
            let error = std::io::Error::last_os_error();
            if error
                .raw_os_error()
                .is_some_and(|code| code == libc::EAGAIN || code == libc::EWOULDBLOCK)
            {
                return Ok(None);
            }
            return Err(DevSyncError::new(
                "lock_error",
                format!("Unable to acquire lock {}: {error}", path.display()),
            ));
        }

        #[cfg(not(unix))]
        {
            Ok(Some(Self { file }))
        }
    }
}

impl Drop for ProcessLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            unsafe {
                libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}
