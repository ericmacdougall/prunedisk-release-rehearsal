//! The only place that runs `git`.
//!
//! Every call goes through [`Git::run_bytes`] (or [`Git::run`], its text form), which returns a
//! typed [`GitError`] for a missing binary, a timeout or a failed command, so a failure can never
//! be mistaken for empty output (principle 2: unknown means keep). Only read-only subcommands are
//! issued by callers; the environment is set so git never takes optional locks and never prompts.
//!
//! Git prints file names as bytes. Output that names paths is read with [`Git::run_bytes`] and
//! turned into paths with [`path_from_git`], which is exact on Unix and refuses (rather than
//! guesses) where it cannot be; [`Git::run`] refuses output that is not valid UTF-8. A lossy
//! decoding is never used to look anything up or to compare.

use crate::redact::redact_text;
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Why a git invocation did not produce output.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitError {
    /// The binary could not be started (missing, not executable).
    #[error("could not start git: {0}")]
    Spawn(String),
    /// The command ran longer than the timeout and was killed.
    #[error("git timed out after {0:?}")]
    Timeout(Duration),
    /// The command exited with a failure status.
    #[error("git exited with status {code:?}: {stderr}")]
    Failed { code: Option<i32>, stderr: String },
    /// Text was expected and git printed bytes that are not valid UTF-8 (a ref, remote or file
    /// name this tool cannot represent exactly).
    #[error("git printed output that is not valid UTF-8")]
    NotUtf8,
}

/// A path git printed, converted exactly: the bytes themselves on Unix; on other platforms (where
/// git prints UTF-8 and paths are not bytes) the decoded text. `None` when the bytes cannot be a
/// path here, which the caller must treat as a failed check.
pub fn path_from_git(bytes: &[u8]) -> Option<PathBuf> {
    if bytes.is_empty() || bytes.contains(&0) {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(PathBuf::from(OsStr::from_bytes(bytes)))
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(bytes).ok().map(PathBuf::from)
    }
}

/// The one path a command such as `rev-parse --show-toplevel` prints: its bytes without the
/// line ending (exactly one `\n`, or `\r\n` on Windows), never trimmed further, so a name that
/// ends in a space stays exact. `None` for empty or unterminated output.
pub fn single_path(out: &[u8]) -> Option<PathBuf> {
    let line = out.strip_suffix(b"\n")?;
    #[cfg(windows)]
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    path_from_git(line)
}

/// `path` relative to `base` in the form git prints it: components joined by `/`, as bytes. Exact
/// on Unix; elsewhere `None` when a component is not valid UTF-8. `None` too when `path` is not
/// below `base`.
pub fn rel_git_bytes(path: &Path, base: &Path) -> Option<Vec<u8>> {
    let rel = path.strip_prefix(base).ok()?;
    let mut out: Vec<u8> = Vec::new();
    for (k, c) in rel.components().enumerate() {
        if k > 0 {
            out.push(b'/');
        }
        out.extend_from_slice(os_bytes(c.as_os_str())?);
    }
    Some(out)
}

#[cfg(unix)]
fn os_bytes(s: &OsStr) -> Option<&[u8]> {
    use std::os::unix::ffi::OsStrExt;
    Some(s.as_bytes())
}

#[cfg(not(unix))]
fn os_bytes(s: &OsStr) -> Option<&[u8]> {
    s.to_str().map(str::as_bytes)
}

/// Kill a process and everything it started (its process group on Unix, `taskkill /T` on
/// Windows). Uses the system tools because this crate forbids unsafe code.
fn kill_tree(pid: u32) {
    #[cfg(unix)]
    {
        let group = format!("-{pid}");
        let _ = Command::new("kill")
            .args(["-9", "--", &group])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// A git binary plus a per-command timeout.
#[derive(Debug, Clone)]
pub struct Git {
    program: OsString,
    timeout: Duration,
}

impl Default for Git {
    fn default() -> Self {
        Git::new("git", Duration::from_secs(30))
    }
}

/// The null device. Git reads it as an empty config file; Git for Windows maps this spelling to
/// `NUL`, and where a git does not, the path does not exist and is skipped like a missing file.
const NULL_DEVICE: &str = "/dev/null";

/// Removed from git's environment even when they are not set in ours (so the list is also what
/// the unit test checks). Every other `GIT_*` variable that is set is removed as well.
const CLEARED_VARS: [&str; 22] = [
    // Redirect git away from the directory we point it at.
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
    // Add configuration that the repo config check never sees.
    "GIT_CONFIG",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_PARAMETERS",
    // Name programs, or let git write files of the caller's choosing.
    "GIT_EXTERNAL_DIFF",
    "GIT_PAGER",
    "GIT_EDITOR",
    "GIT_ASKPASS",
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "GIT_EXEC_PATH",
    "GIT_TRACE",
    "GIT_TRACE2",
    "PAGER",
    "SSH_ASKPASS",
];

/// Should this variable of the caller's environment be withheld from git?
fn withheld(name: &std::ffi::OsStr) -> bool {
    let upper = name.to_string_lossy().to_ascii_uppercase();
    upper.starts_with("GIT_") || CLEARED_VARS.contains(&upper.as_str())
}

impl Git {
    /// Use `program` (a name on `PATH` or a path) with a per-command timeout.
    pub fn new(program: impl Into<OsString>, timeout: Duration) -> Git {
        Git {
            program: program.into(),
            timeout,
        }
    }

    /// The configured timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// The command [`Git::run`] starts, before it is spawned.
    ///
    /// Git reads configuration from the repository (checked against an allowlist by the caller)
    /// and also from the system file, the user's global files and the environment, any of which
    /// can name a program (a `filter.<driver>.clean` selected by the scanned repo's
    /// `.gitattributes`, as Git LFS installs globally). None of those is checked, so none is
    /// read: the system file is off, the global file and `HOME`/`XDG_CONFIG_HOME` point at the
    /// null device (older git ignores `GIT_CONFIG_GLOBAL` but still finds nothing under `HOME`),
    /// and every `GIT_*` variable of the caller is withheld. Only the `-c` values below apply on
    /// top of the repository's own config.
    pub fn command<S: AsRef<OsStr>>(&self, cwd: &Path, args: &[S]) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.arg("-c")
            .arg("core.fsmonitor=false")
            .arg("-c")
            .arg("gc.auto=0")
            .arg("-c")
            .arg("log.showSignature=false")
            .arg("-c")
            .arg("core.ignoreStat=false")
            // No hook of the repo's runs even if some command wrote the index.
            .arg("-c")
            .arg(format!("core.hooksPath={NULL_DEVICE}"))
            // No transport (a lazy fetch in a partial clone on a git without
            // GIT_NO_LAZY_FETCH would start ssh or a remote helper).
            .arg("-c")
            .arg("protocol.allow=never")
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, _) in std::env::vars_os() {
            if withheld(&name) {
                cmd.env_remove(name);
            }
        }
        for var in CLEARED_VARS {
            cmd.env_remove(var);
        }
        cmd.env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_SYSTEM", NULL_DEVICE)
            .env("GIT_CONFIG_GLOBAL", NULL_DEVICE)
            .env("HOME", NULL_DEVICE)
            .env("XDG_CONFIG_HOME", NULL_DEVICE)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("LC_ALL", "C");
        cmd
    }

    /// Run `git <args>` in `cwd` and return stdout as text, isolated from configuration the
    /// caller did not check (see [`Git::command`]). Output that is not valid UTF-8 is
    /// [`GitError::NotUtf8`], never decoded lossily.
    pub fn run(&self, cwd: &Path, args: &[&str]) -> Result<String, GitError> {
        String::from_utf8(self.run_bytes(cwd, args)?).map_err(|_| GitError::NotUtf8)
    }

    /// Run `git <args>` in `cwd` and return stdout as bytes (for output that names paths). The
    /// arguments may be any OS strings, so a path is passed exactly.
    pub fn run_bytes<S: AsRef<OsStr>>(&self, cwd: &Path, args: &[S]) -> Result<Vec<u8>, GitError> {
        let mut cmd = self.command(cwd, args);
        // Own process group, so a timeout can kill git and everything it started.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| GitError::Spawn(redact_text(&e.to_string())))?;

        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let out_reader = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(s) = stdout.as_mut() {
                let _ = s.read_to_end(&mut buf);
            }
            buf
        });
        let err_reader = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(s) = stderr.as_mut() {
                let _ = s.read_to_end(&mut buf);
            }
            buf
        });

        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        kill_tree(child.id());
                        let _ = child.kill();
                        let _ = child.wait();
                        // Do not join the readers: a grandchild may still hold the pipes.
                        return Err(GitError::Timeout(self.timeout));
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(e) => {
                    kill_tree(child.id());
                    let _ = child.kill();
                    return Err(GitError::Spawn(redact_text(&e.to_string())));
                }
            }
        };

        let out = out_reader.join().unwrap_or_default();
        let err = err_reader.join().unwrap_or_default();
        if status.success() {
            Ok(out)
        } else {
            let stderr: String = redact_text(String::from_utf8_lossy(&err).trim())
                .chars()
                .take(300)
                .collect();
            Err(GitError::Failed {
                code: status.code(),
                stderr,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsStr;

    #[test]
    fn inherited_configuration_and_redirects_are_withheld() {
        let cmd = Git::default().command(Path::new("."), &["status"]);
        let envs: HashMap<String, Option<String>> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().to_ascii_uppercase(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        let set = |k: &str| envs.get(k).cloned().flatten();
        assert_eq!(set("GIT_CONFIG_NOSYSTEM").as_deref(), Some("1"));
        for k in [
            "GIT_CONFIG_SYSTEM",
            "GIT_CONFIG_GLOBAL",
            "HOME",
            "XDG_CONFIG_HOME",
        ] {
            assert_eq!(set(k).as_deref(), Some(NULL_DEVICE), "{k}");
        }
        assert_eq!(set("GIT_OPTIONAL_LOCKS").as_deref(), Some("0"));
        for k in [
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_PARAMETERS",
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_EXTERNAL_DIFF",
            "GIT_SSH_COMMAND",
            "GIT_EXEC_PATH",
        ] {
            assert_eq!(envs.get(k), Some(&None), "{k} must be removed");
        }
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        for a in [
            "core.fsmonitor=false",
            "protocol.allow=never",
            "core.hooksPath=/dev/null",
        ] {
            assert!(args.iter().any(|x| x == a), "{a} missing from {args:?}");
        }
    }

    #[test]
    fn a_printed_path_loses_only_its_line_ending() {
        assert_eq!(single_path(b"/w/a b \n"), Some(PathBuf::from("/w/a b ")));
        assert_eq!(single_path(b"/w/x\n\n"), Some(PathBuf::from("/w/x\n")));
        assert_eq!(single_path(b"/w/a"), None);
        assert_eq!(single_path(b"\n"), None);
        assert_eq!(path_from_git(b"a\0b"), None);
        let rel = rel_git_bytes(Path::new("/w/a/b/c"), Path::new("/w/a"));
        assert_eq!(rel.as_deref(), Some(&b"b/c"[..]));
        assert_eq!(rel_git_bytes(Path::new("/x"), Path::new("/w")), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_printed_path_that_is_not_utf8_is_kept_byte_for_byte() {
        use std::os::unix::ffi::OsStrExt;
        let p = single_path(b"/w/n\xFFme\n").unwrap();
        assert_eq!(p.as_os_str().as_bytes(), b"/w/n\xFFme");
        let base = Path::new("/w");
        assert_eq!(rel_git_bytes(&p, base).unwrap(), b"n\xFFme".to_vec());
    }

    #[test]
    fn every_git_variable_of_the_caller_is_withheld() {
        for k in ["GIT_TRACE_PACKET", "GIT_CONFIG_KEY_0", "git_dir", "PAGER"] {
            assert!(withheld(OsStr::new(k)), "{k}");
        }
        for k in ["PATH", "LANG", "TMPDIR"] {
            assert!(!withheld(OsStr::new(k)), "{k}");
        }
    }
}
