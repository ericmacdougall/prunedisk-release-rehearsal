//! Opening a file the scan has already seen as regular, without trusting that it still is.
//!
//! Every reader `lstat`s an entry before it opens it, but the entry can be replaced in between.
//! On unix the open is non-blocking (`O_NONBLOCK`), so a FIFO swapped in is opened at once
//! instead of waiting for a writer, and does not follow a final symlink (`O_NOFOLLOW`, which
//! fails with `ELOOP`); the open file is then checked with `fstat` and refused unless it is a
//! regular file, before anything is read. `O_NONBLOCK` has no effect on reading a regular file.
//!
//! On Windows there are no FIFOs in the file system: named pipes live only under `\\.\pipe\`,
//! and opening one never waits for a server. The open uses std's defaults (a final symlink is
//! followed, as before, and making one needs a privilege or developer mode); the same check on
//! the open handle refuses anything that is not a regular file before it is read.

use std::fs::File;
use std::io;
use std::path::Path;

/// Open `path` for reading if it is, at the moment of opening, a regular file. Anything else is
/// an error whose text is `not a regular file`; an open that fails keeps its own error.
pub(crate) fn open_regular(path: &Path) -> io::Result<File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = opts.open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(file)
}
