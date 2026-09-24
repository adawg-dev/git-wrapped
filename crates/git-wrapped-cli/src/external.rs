//! Optional third-party companions. Nothing here runs unless explicitly requested.
pub mod fame;

use std::{
    fs,
    io::{ErrorKind, Read},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

const TOOLS: &[(&str, &str)] = &[
    ("gource", "MP4 renderer"),
    ("ffmpeg", "MP4 encoder"),
    ("git-fame", "JSON comparison"),
    ("git-of-theseus-analyze", "cohort capture"),
    ("hercules", "manual companion"),
    ("git-quick-stats", "manual companion"),
];

#[derive(Clone, Debug)]
pub struct ToolStatus {
    pub id: &'static str,
    pub installed: bool,
    pub version: Option<String>,
    pub integration: &'static str,
}

/// Locate an executable regular file named `name` on PATH.
pub fn find_executable(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

/// The executable for an explicitly requested tool, or an actionable error.
pub fn require(name: &str) -> Result<PathBuf, String> {
    find_executable(name).ok_or_else(|| {
        format!("{name} is not installed or not executable on PATH; install it, then check `git-wrapped external list`")
    })
}

/// Refuse symlinks or non-directories along `dir` without creating anything.
pub(crate) fn check_target(dir: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for part in dir.components() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!(
                    "refusing symlink output directory: {}",
                    current.display()
                ))
            }
            Ok(meta) if !meta.is_dir() => {
                return Err(format!("output is not a directory: {}", current.display()))
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("inspect {}: {error}", current.display())),
        }
    }
    Ok(())
}

pub(crate) fn head_sha(root: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || sha.is_empty() {
        return Err("repository has no commits".into());
    }
    Ok(sha)
}

/// A nonzero-exit error carrying the tool's (capped, sanitized) stderr.
pub(crate) fn failure(tool: &str, captured: &Captured) -> String {
    let stderr: String = String::from_utf8_lossy(&captured.stderr)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    format!("{tool} failed ({}): {}", captured.status, stderr.trim())
}

pub fn discover_tools() -> Vec<ToolStatus> {
    TOOLS
        .iter()
        .map(|&(id, integration)| {
            let executable = find_executable(id);
            ToolStatus {
                id,
                installed: executable.is_some(),
                version: executable.and_then(|path| version(&path)),
                integration,
            }
        })
        .collect()
}

/// One bounded `--version` call; any failure means "unknown", never an analysis run.
pub(crate) fn version(executable: &Path) -> Option<String> {
    let mut command = Command::new(executable);
    command.arg("--version");
    let output = run_capped(command, 4096, Some(Duration::from_secs(5))).ok()?;
    if !output.status.success() || output.stdout_overflow {
        return None;
    }
    let text = if output.stdout.iter().all(u8::is_ascii_whitespace) {
        output.stderr
    } else {
        output.stdout
    };
    let line = String::from_utf8_lossy(&text)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect::<String>();
    // Keep the version token when a tool prints "<name> <version>".
    let token = line
        .split_whitespace()
        .find(|word| {
            word.trim_start_matches('v')
                .starts_with(|c: char| c.is_ascii_digit())
        })
        .unwrap_or(&line);
    let token: String = token
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || ".-+_".contains(*c))
        .collect();
    (!token.is_empty()).then_some(token)
}

pub(crate) struct Captured {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stdout_overflow: bool,
    pub stderr: Vec<u8>,
}

/// Keep at most `limit` bytes, draining the rest so the child never blocks on a full pipe.
fn read_capped(mut reader: impl Read, limit: usize) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let mut overflow = false;
    let mut buffer = [0; 64 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = limit.saturating_sub(kept.len());
                kept.extend_from_slice(&buffer[..n.min(room)]);
                overflow |= n > room;
            }
        }
    }
    (kept, overflow)
}

/// Run a fixed command with null stdin, capped stdout (`stdout_limit`) and stderr (4 KB).
pub(crate) fn run_capped(
    mut command: Command,
    stdout_limit: usize,
    timeout: Option<Duration>,
) -> Result<Captured, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {:?}: {e}", command.get_program()))?;
    let stdout = child.stdout.take().ok_or("missing stdout pipe")?;
    let stderr = child.stderr.take().ok_or("missing stderr pipe")?;
    let stdout = std::thread::spawn(move || read_capped(stdout, stdout_limit));
    let stderr = std::thread::spawn(move || read_capped(stderr, 4096).0);
    let status = match timeout {
        None => child.wait().map_err(|e| e.to_string())?,
        Some(timeout) => {
            let deadline = Instant::now() + timeout;
            loop {
                if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                    break status;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{:?} timed out", command.get_program()));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    let (stdout, stdout_overflow) = stdout.join().map_err(|_| "stdout reader panicked")?;
    let stderr = stderr.join().map_err(|_| "stderr reader panicked")?;
    Ok(Captured {
        status,
        stdout,
        stdout_overflow,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capped_reads_drain_and_flag_overflow() {
        assert_eq!(read_capped(&b"abc"[..], 3), (b"abc".to_vec(), false));
        assert_eq!(read_capped(&b"abcd"[..], 3), (b"abc".to_vec(), true));
    }

    #[test]
    fn version_keeps_sanitized_version_token() {
        let dir = std::env::temp_dir().join(format!("gw-version-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tool = dir.join("tool");
        std::fs::write(
            &tool,
            "#!/bin/sh\nprintf 'tool version v2.0\\033[0m\\nextra\\n'\n",
        )
        .unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(version(&tool).as_deref(), Some("v2.0"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
