//! Running processes' working directories, used as activity evidence (design section 3,
//! Activity). Read-only. Failure is reported as `Unknown`, never as "nothing is running".

use std::path::{Path, PathBuf};

/// Result of looking for processes whose working directory is inside a project.
#[derive(Debug, Clone)]
pub enum ProcessCheck {
    /// Every process's working directory was readable.
    Ok { cwds: Vec<PathBuf> },
    /// Some processes could not be inspected (usually other users' processes).
    Partial {
        cwds: Vec<PathBuf>,
        unreadable: usize,
    },
    /// Process information could not be read at all.
    Unknown { reason: String },
}

impl ProcessCheck {
    /// An empty, fully-known process list (tests and callers that checked by other means).
    pub fn none_running() -> ProcessCheck {
        ProcessCheck::Ok { cwds: Vec::new() }
    }

    /// Short name for reports and the plan header.
    pub fn status(&self) -> &'static str {
        match self {
            ProcessCheck::Ok { .. } => "ok",
            ProcessCheck::Partial { .. } => "partial",
            ProcessCheck::Unknown { .. } => "unknown",
        }
    }

    /// Why the check is not `ok`.
    pub fn note(&self) -> Option<String> {
        match self {
            ProcessCheck::Ok { .. } => None,
            ProcessCheck::Partial { unreadable, .. } => Some(format!(
                "{} not be inspected; a project {} may look idle",
                crate::words::count(*unreadable as u64, "process could", "processes could"),
                crate::words::agree(*unreadable as u64, "it uses", "they use")
            )),
            ProcessCheck::Unknown { reason } => Some(reason.clone()),
        }
    }

    /// Working directories seen.
    pub fn cwds(&self) -> &[PathBuf] {
        match self {
            ProcessCheck::Ok { cwds } | ProcessCheck::Partial { cwds, .. } => cwds,
            ProcessCheck::Unknown { .. } => &[],
        }
    }

    /// First working directory inside `dir`, if any.
    pub fn inside(&self, dir: &Path) -> Option<&Path> {
        self.cwds()
            .iter()
            .map(|p| p.as_path())
            .find(|c| c.starts_with(dir))
    }
}

/// Read the working directory of every running process.
pub fn running_cwds() -> ProcessCheck {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    if !sysinfo::IS_SUPPORTED_SYSTEM {
        return ProcessCheck::Unknown {
            reason: "process information is not supported on this platform".to_string(),
        };
    }
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always),
    );
    let mut cwds = Vec::new();
    let mut unreadable = 0usize;
    for p in sys.processes().values() {
        match p.cwd() {
            Some(c) => cwds.push(c.to_path_buf()),
            None => unreadable += 1,
        }
    }
    if cwds.is_empty() {
        ProcessCheck::Unknown {
            reason: "no process working directory could be read".to_string(),
        }
    } else if unreadable > 0 {
        ProcessCheck::Partial { cwds, unreadable }
    } else {
        ProcessCheck::Ok { cwds }
    }
}
