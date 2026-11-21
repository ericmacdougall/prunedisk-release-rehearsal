//! Test support: real FIFOs, and a bound on calls that could block opening one.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// How long a read of a FIFO may take before the test counts it as blocked.
const LIMIT: Duration = Duration::from_secs(3);

/// Make a FIFO at `path` with the system's `mkfifo`.
pub(crate) fn mkfifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo {}", path.display());
}

/// Run `read` on its own thread and return its result. If it has not returned within [`LIMIT`]
/// it is blocked (in `open`, waiting for a writer): the FIFO is opened for reading and writing,
/// which does not block, and closed again, so the blocked open returns and its read sees the end;
/// the test then fails at once instead of hanging the job. (A thread still blocked would not keep
/// the test binary from exiting either.)
pub(crate) fn must_not_block<T: Send + 'static>(
    fifo: &Path,
    read: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(read());
    });
    match rx.recv_timeout(LIMIT) {
        Ok(v) => v,
        Err(_) => {
            let fifo: PathBuf = fifo.to_path_buf();
            let release = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&fifo);
            std::thread::sleep(Duration::from_millis(200));
            drop(release);
            let _ = rx.recv_timeout(LIMIT);
            panic!(
                "reading {} blocked for more than {LIMIT:?} on a FIFO",
                fifo.display()
            );
        }
    }
}
