use crate::model::{FileChange, Identity, RawCommit, TagDate};
use crate::progress::CancelFlag;
use chrono::DateTime;
use std::{
    ffi::{OsStr, OsString},
    io::{BufRead, BufReader},
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

#[derive(Debug)]
pub struct Repository {
    pub root: PathBuf,
    pub name: String,
    pub shallow: bool,
    pub tracked_files: usize,
}

fn git(path: &Path, args: &[&OsStr]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

pub fn discover(path: &Path) -> Result<Repository, String> {
    let root = git(
        path,
        &[OsStr::new("rev-parse"), OsStr::new("--show-toplevel")],
    )?;
    let root = PathBuf::from(std::ffi::OsString::from_vec(
        root.strip_suffix(b"\n").unwrap_or(&root).to_vec(),
    ));
    let head = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !head.status.success() {
        return Err("repository has no commits".into());
    }
    let shallow = git(
        &root,
        &[
            OsStr::new("rev-parse"),
            OsStr::new("--is-shallow-repository"),
        ],
    )? == b"true\n";
    let files = git(
        &root,
        &[
            OsStr::new("ls-tree"),
            OsStr::new("-r"),
            OsStr::new("-z"),
            OsStr::new("--name-only"),
            OsStr::new("HEAD"),
        ],
    )?;
    let tracked_files = files.iter().filter(|&&b| b == 0).count();
    let name = root
        .file_name()
        .unwrap_or(root.as_os_str())
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    Ok(Repository {
        root,
        name,
        shallow,
        tracked_files,
    })
}

pub fn head_paths(repo: &Repository) -> Result<Vec<Vec<u8>>, String> {
    tree_paths(repo, "HEAD")
}

pub(crate) fn tree_paths(repo: &Repository, sha: &str) -> Result<Vec<Vec<u8>>, String> {
    let bytes = git(
        &repo.root,
        &[
            OsStr::new("ls-tree"),
            OsStr::new("-r"),
            OsStr::new("-z"),
            OsStr::new("--name-only"),
            OsStr::new(sha),
        ],
    )?;
    Ok(bytes
        .split(|&byte| byte == 0)
        .filter(|path| !path.is_empty())
        .map(Vec::from)
        .collect())
}

pub fn reachable_tag_dates(repo: &Repository) -> Result<Vec<TagDate>, String> {
    let names = git(
        &repo.root,
        &[
            OsStr::new("tag"),
            OsStr::new("--merged"),
            OsStr::new("HEAD"),
            OsStr::new("--list"),
        ],
    )?;
    let mut tags = Vec::new();
    for name in names
        .split(|&byte| byte == b'\n')
        .filter(|name| !name.is_empty())
    {
        let mut reference = b"refs/tags/".to_vec();
        reference.extend_from_slice(name);
        reference.extend_from_slice(b"^{commit}");
        let reference = OsString::from_vec(reference);
        let sha = match git(
            &repo.root,
            &[OsStr::new("rev-parse"), OsStr::new("--verify"), &reference],
        ) {
            Ok(bytes) => parse_sha(bytes.strip_suffix(b"\n").unwrap_or(&bytes))?,
            Err(_) => continue, // A reachable tag may point at a noncommit object.
        };
        let bytes = git(
            &repo.root,
            &[
                OsStr::new("log"),
                OsStr::new("-1"),
                OsStr::new("--format=%cI"),
                &reference,
            ],
        )?;
        let committer_time = String::from_utf8(bytes)
            .map_err(|e| e.to_string())?
            .trim_end_matches('\n')
            .to_owned();
        let date = DateTime::parse_from_rfc3339(&committer_time).map_err(|e| e.to_string())?;
        tags.push((
            date,
            TagDate {
                name: String::from_utf8_lossy(name).into_owned(),
                target_sha: sha,
                committer_time,
            },
        ));
    }
    tags.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    Ok(tags.into_iter().map(|(_, tag)| tag).collect())
}

fn token<R: BufRead>(reader: &mut R) -> Result<Option<Vec<u8>>, String> {
    let mut bytes = Vec::new();
    let n = reader
        .read_until(0, &mut bytes)
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(None);
    }
    if bytes.pop() != Some(0) {
        return Err("unterminated git log field".into());
    }
    Ok(Some(bytes))
}

fn required<R: BufRead>(reader: &mut R) -> Result<Vec<u8>, String> {
    token(reader)?.ok_or_else(|| "truncated git log".into())
}

fn field(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).into_owned()
}

fn parse_sha(bytes: &[u8]) -> Result<String, String> {
    if ![40, 64].contains(&bytes.len()) || !bytes.iter().all(u8::is_ascii_hexdigit) {
        return Err("invalid commit SHA in git log".into());
    }
    Ok(field(bytes.to_vec()))
}

fn metadata<R: BufRead>(first: &[u8], reader: &mut R) -> Result<RawCommit, String> {
    let sha = parse_sha(
        first
            .strip_prefix(&[0x1e])
            .ok_or("missing commit separator")?,
    )?;
    let parents = required(reader)?;
    let parents = if parents.is_empty() {
        Vec::new()
    } else {
        parents
            .split(|&b| b == b' ')
            .map(parse_sha)
            .collect::<Result<Vec<_>, _>>()?
    };
    let raw_author = Identity {
        name: field(required(reader)?),
        email: field(required(reader)?),
    };
    let mapped_author = Identity {
        name: field(required(reader)?),
        email: field(required(reader)?),
    };
    let author_time = field(required(reader)?);
    let committer_time = field(required(reader)?);
    let subject = field(required(reader)?);
    Ok(RawCommit {
        sha,
        parents,
        raw_author,
        mapped_author,
        author_time,
        committer_time,
        subject,
        changes: Vec::new(),
    })
}

fn change(item: &[u8]) -> Result<FileChange, String> {
    let mut parts = item.splitn(3, |&b| b == b'\t');
    let a = parts.next().ok_or("missing additions")?;
    let d = parts.next().ok_or("missing deletions")?;
    let path = parts.next().ok_or("missing path")?;
    let binary = a == b"-" && d == b"-";
    if !binary && (a == b"-" || d == b"-") {
        return Err("invalid binary numstat".into());
    }
    let count = |s: &[u8]| -> Result<u64, String> {
        if s.is_empty() || !s.iter().all(u8::is_ascii_digit) {
            return Err("invalid numstat count".into());
        }
        std::str::from_utf8(s)
            .unwrap()
            .parse()
            .map_err(|_| "numstat count overflow".into())
    };
    Ok(FileChange {
        path: path.to_vec(),
        old_path: None,
        additions: if binary { 0 } else { count(a)? },
        deletions: if binary { 0 } else { count(d)? },
        binary,
    })
}

fn parse<R: BufRead>(
    reader: &mut R,
    visit: &mut impl FnMut(RawCommit) -> Result<(), String>,
) -> Result<(), String> {
    let mut current: Option<RawCommit> = None;
    while let Some(mut item) = token(reader)? {
        if item.first() == Some(&0x1e) {
            if let Some(mut commit) = current.take() {
                if commit.parents.len() > 1 {
                    commit.changes.clear();
                }
                visit(commit)?;
            }
            current = Some(metadata(&item, reader)?);
            continue;
        }
        if item.is_empty() {
            continue;
        }
        if item.first() == Some(&b'\n') {
            item.remove(0);
        }
        if item.is_empty() {
            continue;
        }
        let commit = current.as_mut().ok_or("numstat before commit metadata")?;
        let mut change = change(&item)?;
        if change.path.is_empty() {
            change.old_path = Some(required(reader)?);
            change.path = required(reader)?;
        }
        commit.changes.push(change);
    }
    if let Some(mut commit) = current {
        if commit.parents.len() > 1 {
            commit.changes.clear();
        }
        visit(commit)?;
    }
    Ok(())
}

pub fn scan(
    repo: &Repository,
    visit: impl FnMut(RawCommit) -> Result<(), String>,
) -> Result<(), String> {
    scan_with_cancel(repo, &CancelFlag::default(), visit)
}

pub fn scan_with_cancel(
    repo: &Repository,
    cancel: &CancelFlag,
    visit: impl FnMut(RawCommit) -> Result<(), String>,
) -> Result<(), String> {
    cancel.check()?;
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(&repo.root)
        .args([
            "--no-pager",
            "log",
            "HEAD",
            "--root",
            "--no-ext-diff",
            "--no-show-signature",
            "--no-textconv",
            "--find-renames",
            "--numstat",
            "-z",
            "--format=%x1e%H%x00%P%x00%an%x00%ae%x00%aN%x00%aE%x00%aI%x00%cI%x00%s%x00",
        ])
        .env("GIT_NO_REPLACE_OBJECTS", "1");
    scan_command_with_cancel(command, cancel, visit)
}

fn scan_command_with_cancel(
    mut command: Command,
    cancel: &CancelFlag,
    mut visit: impl FnMut(RawCommit) -> Result<(), String>,
) -> Result<(), String> {
    cancel.check()?;
    let mut child = command
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run git log: {e}"))?;
    let stdout = child.stdout.take().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let watcher_stop = Arc::clone(&stop);
    let watcher_flag = cancel.clone();
    let watcher = thread::spawn(move || -> Result<ExitStatus, String> {
        loop {
            if watcher_flag.is_cancelled() || watcher_stop.load(Ordering::SeqCst) {
                let _ = child.kill();
                return child.wait().map_err(|e| e.to_string());
            }
            match child.try_wait() {
                Ok(Some(status)) => return Ok(status),
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error.to_string());
                }
            }
        }
    });
    let result = parse(&mut BufReader::new(stdout), &mut |commit| {
        cancel.check()?;
        visit(commit)
    });
    if result.is_err() {
        stop.store(true, Ordering::SeqCst);
    }
    let outcome = watcher.join();
    cancel.check()?;
    let status = outcome.map_err(|_| "git log watcher panicked".to_string())??;
    result?;
    if !status.success() {
        return Err(format!("git log exited with {status}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn cancelled_scan_interrupts_and_reaps_a_stalled_git_child() {
        use std::{sync::mpsc, thread, time::Duration};

        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("child.pid");
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                "echo $$ > \"$1\"; printf '\\036'; exec sleep 30",
                "sh",
            ])
            .arg(&pid_file);
        let flag = CancelFlag::default();
        let scan_flag = flag.clone();
        let (sender, receiver) = mpsc::channel();
        let scan = thread::spawn(move || {
            sender
                .send(scan_command_with_cancel(command, &scan_flag, |_| Ok(())))
                .unwrap();
        });
        let pid = (0..100)
            .find_map(|_| {
                let pid = std::fs::read_to_string(&pid_file)
                    .ok()
                    .and_then(|pid| pid.trim().parse::<u32>().ok());
                if pid.is_none() {
                    thread::sleep(Duration::from_millis(10));
                }
                pid
            })
            .expect("fake Git child never started");
        thread::sleep(Duration::from_millis(30));
        flag.cancel();
        let result = receiver.recv_timeout(Duration::from_secs(2));
        if result.is_err() {
            let _ = Command::new("kill")
                .arg(pid.to_string())
                .stderr(Stdio::null())
                .status();
        }
        assert!(result
            .expect("scan did not stop promptly")
            .unwrap_err()
            .contains("cancelled"));
        scan.join().unwrap();
        assert!(!Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success());
    }

    fn record(changes: &[u8]) -> Vec<u8> {
        let mut bytes = format!("\x1e{}\0\0Test\0test@example.com\0Test\0test@example.com\02024-01-01T10:00:00+00:00\02024-01-01T10:00:00+00:00\0subject\0\0\n", "a".repeat(40)).into_bytes();
        bytes.extend_from_slice(changes);
        bytes
    }

    #[test]
    fn stream_preserves_non_utf8_ordinary_path() {
        let bytes = record(b"4\t2\todd\xff\nname.rs\0-\t-\tphoto.png\0");
        let mut commits = Vec::new();
        parse(&mut std::io::Cursor::new(bytes), &mut |commit| {
            commits.push(commit);
            Ok(())
        })
        .unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].changes[0].path, b"odd\xff\nname.rs");
        assert_eq!(
            (
                commits[0].changes[0].additions,
                commits[0].changes[0].deletions
            ),
            (4, 2)
        );
        assert!(commits[0].changes[1].binary);
    }

    #[test]
    fn stream_preserves_non_utf8_rename_paths() {
        let bytes = record(b"0\t0\t\0old\xff.rs\0new\xfe.rs\0");
        let mut commits = Vec::new();
        parse(&mut std::io::Cursor::new(bytes), &mut |commit| {
            commits.push(commit);
            Ok(())
        })
        .unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(
            commits[0].changes[0].old_path.as_deref(),
            Some(b"old\xff.rs".as_slice())
        );
        assert_eq!(commits[0].changes[0].path, b"new\xfe.rs");
    }
}
