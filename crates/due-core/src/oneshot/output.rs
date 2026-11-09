use std::{
    fs::OpenOptions,
    io::{self, Read},
    path::Path,
};

pub(super) const STREAM_LIMIT: usize = 64 * 1024 * 1024;
pub(super) const FINAL_LIMIT: usize = 4 * 1024 * 1024;

pub(crate) use crate::diagnostics::diagnostic;

pub(super) fn final_message(path: &Path) -> io::Result<String> {
    read_final_with_limit(path, FINAL_LIMIT)
}

fn read_final_with_limit(path: &Path, limit: usize) -> io::Result<String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "final-message target is not a regular file",
        ));
    }
    // Metadata is only an early rejection: the file may grow after opening.
    let mut bytes = Vec::new();
    file.by_ref()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("final-message file exceeds {limit} bytes"),
        ));
    }
    String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn final_file_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("answer");
        std::fs::write(&path, "abcd").unwrap();
        assert_eq!(read_final_with_limit(&path, 4).unwrap(), "abcd");
        assert!(read_final_with_limit(&path, 3).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlink_and_fifo() {
        use std::os::unix::{ffi::OsStrExt, fs::symlink};
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, "answer").unwrap();
        let link = dir.path().join("link");
        symlink(&target, &link).unwrap();
        assert!(final_message(&link).is_err());
        let fifo = dir.path().join("fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(final_message(&fifo).is_err());
    }
}
