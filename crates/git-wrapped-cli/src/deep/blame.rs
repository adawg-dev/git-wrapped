use crate::progress::CancelFlag;
use crate::{config::Config, git::Repository, model::Identity};
use std::{
    collections::BTreeMap,
    io::{self, BufRead, BufReader, Write},
    os::unix::ffi::OsStringExt,
    process::{Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub(crate) struct BlamedLine {
    pub sha: String,
    pub original_line: u64,
    pub final_line: u64,
    pub filename: Vec<u8>,
    pub author: Option<Identity>,
    pub author_time: Option<i64>,
    pub author_tz: Option<String>,
}

pub(crate) fn blame(
    repo: &Repository,
    path: &[u8],
    max_lines: u64,
    cancel: &CancelFlag,
    deadline: Option<Instant>,
) -> Result<(Vec<BlamedLine>, bool), String> {
    blame_at(repo, "HEAD", path, max_lines, cancel, deadline)
}

pub(crate) fn blame_at(
    repo: &Repository,
    revision: &str,
    path: &[u8],
    max_lines: u64,
    cancel: &CancelFlag,
    deadline: Option<Instant>,
) -> Result<(Vec<BlamedLine>, bool), String> {
    if revision != "HEAD" && !super::valid_sha(revision) {
        return Err("invalid blame revision".into());
    }
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(&repo.root)
        .args(["blame", "--line-porcelain", "--root", "--no-textconv"])
        .arg(revision)
        .arg("--")
        .arg(std::ffi::OsString::from_vec(path.to_vec()));
    blame_command_with_cancel(command, max_lines, cancel, deadline, path)
}

fn decode_filename(value: &[u8]) -> Result<Vec<u8>, String> {
    if value.first() != Some(&b'"') {
        return Ok(value.to_vec());
    }
    if value.last() != Some(&b'"') || value.len() < 2 {
        return Err("invalid quoted blame filename".into());
    }
    let mut decoded = Vec::new();
    let mut index = 1;
    while index < value.len() - 1 {
        if value[index] != b'\\' {
            decoded.push(value[index]);
            index += 1;
            continue;
        }
        index += 1;
        let escaped = *value.get(index).ok_or("incomplete blame filename escape")?;
        if (b'0'..=b'7').contains(&escaped) {
            if index + 2 >= value.len() - 1 {
                return Err("incomplete octal blame filename escape".into());
            }
            let octal = &value[index..index + 3];
            if !octal.iter().all(|byte| (b'0'..=b'7').contains(byte)) {
                return Err("invalid octal blame filename escape".into());
            }
            let byte = u16::from(octal[0] - b'0') * 64
                + u16::from(octal[1] - b'0') * 8
                + u16::from(octal[2] - b'0');
            decoded.push(u8::try_from(byte).map_err(|_| "invalid octal blame filename byte")?);
            index += 3;
        } else {
            decoded.push(match escaped {
                b'a' => 7,
                b'b' => 8,
                b't' => b'\t',
                b'n' => b'\n',
                b'v' => 11,
                b'f' => 12,
                b'r' => b'\r',
                b'\\' => b'\\',
                b'"' => b'"',
                _ => return Err("invalid blame filename escape".into()),
            });
            index += 1;
        }
    }
    Ok(decoded)
}

const MAX_RECORD_BYTES: usize = 1024 * 1024;
const MAX_OUTPUT_BYTES: u64 = 512 * 1024 * 1024;

enum Record {
    Eof,
    Line,
    Limit,
}

fn read_record(
    reader: &mut impl BufRead,
    record: &mut Vec<u8>,
    total: &mut u64,
    cap: u64,
) -> io::Result<Record> {
    record.clear();
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(if record.is_empty() {
                Record::Eof
            } else {
                Record::Line
            });
        }
        let end = buffer
            .iter()
            .position(|&b| b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(buffer.len());
        let allowed = MAX_RECORD_BYTES
            .saturating_sub(record.len())
            .min(cap.saturating_sub(*total) as usize);
        if end > allowed {
            return Ok(Record::Limit);
        }
        let complete = buffer[end - 1] == b'\n';
        record.extend_from_slice(&buffer[..end]);
        reader.consume(end);
        *total += end as u64;
        if complete {
            return Ok(Record::Line);
        }
    }
}

fn parse_header(record: &[u8]) -> Option<(String, u64, u64, bool)> {
    let mut fields = record.split(|&b| b == b' ');
    let token = fields.next()?;
    let boundary = token.first() == Some(&b'^');
    let sha = if boundary { &token[1..] } else { token };
    if !matches!(sha.len(), 40 | 64) || !sha.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let original = std::str::from_utf8(fields.next()?).ok()?.parse().ok()?;
    let final_line = std::str::from_utf8(fields.next()?).ok()?.parse().ok()?;
    Some((
        String::from_utf8(sha.to_vec()).ok()?,
        original,
        final_line,
        boundary,
    ))
}

enum ChildOutcome {
    Exited(ExitStatus),
    Stopped,
    Deadline,
}

fn blame_command_with_cancel(
    mut command: Command,
    max_lines: u64,
    cancel: &CancelFlag,
    deadline: Option<Instant>,
    path: &[u8],
) -> Result<(Vec<BlamedLine>, bool), String> {
    cancel.check()?;
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("git blame: {e}"))?;
    let stdout = child.stdout.take().ok_or("git blame stdout unavailable")?;
    let stop = Arc::new(AtomicBool::new(false));
    let watcher_stop = Arc::clone(&stop);
    let watcher_flag = cancel.clone();
    let watcher = thread::spawn(move || -> Result<ChildOutcome, String> {
        loop {
            let cancelled = watcher_flag.is_cancelled();
            let expired = deadline.is_some_and(|time| Instant::now() >= time);
            if cancelled || expired || watcher_stop.load(Ordering::SeqCst) {
                let _ = child.kill();
                child.wait().map_err(|e| format!("git blame wait: {e}"))?;
                return Ok(if expired {
                    ChildOutcome::Deadline
                } else {
                    ChildOutcome::Stopped
                });
            }
            match child.try_wait() {
                Ok(Some(status)) => return Ok(ChildOutcome::Exited(status)),
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("git blame wait: {error}"));
                }
            }
        }
    });
    let mut reader = BufReader::new(stdout);
    let cap = max_lines
        .saturating_mul(2048)
        .clamp(MAX_RECORD_BYTES as u64, MAX_OUTPUT_BYTES);
    let mut record = Vec::new();
    let mut total = 0;
    let mut lines = Vec::new();
    let mut sha = String::new();
    let mut original_line = 0;
    let mut final_line = 0;
    let mut name = String::new();
    let mut email = String::new();
    let mut author_time = None;
    let mut author_tz = None;
    let mut filename = path.to_vec();
    let mut boundary = false;
    let mut truncated = false;
    let parsed = (|| -> Result<(), String> {
        loop {
            match read_record(&mut reader, &mut record, &mut total, cap)
                .map_err(|e| format!("read git blame: {e}"))?
            {
                Record::Eof => break,
                Record::Limit => {
                    truncated = true;
                    break;
                }
                Record::Line => {}
            }
            let record = record.strip_suffix(b"\n").unwrap_or(&record);
            if let Some(content) = record.strip_prefix(b"\t") {
                if !content.iter().all(u8::is_ascii_whitespace) {
                    if lines.len() as u64 >= max_lines {
                        truncated = true;
                        break;
                    }
                    let author = if boundary || sha.bytes().all(|b| b == b'0') || email.is_empty() {
                        None
                    } else {
                        Some(Identity {
                            name: name.clone(),
                            email: email.clone(),
                        })
                    };
                    lines.push(BlamedLine {
                        sha: sha.clone(),
                        original_line,
                        final_line,
                        filename: filename.clone(),
                        author,
                        author_time,
                        author_tz: author_tz.clone(),
                    });
                }
                continue;
            }
            if let Some((parsed_sha, parsed_line, parsed_final, parsed_boundary)) =
                parse_header(record)
            {
                sha = parsed_sha;
                original_line = parsed_line;
                final_line = parsed_final;
                name.clear();
                email.clear();
                author_time = None;
                author_tz = None;
                filename = path.to_vec();
                boundary = parsed_boundary;
            } else if let Some(value) = record.strip_prefix(b"author ") {
                name = String::from_utf8_lossy(value).into_owned();
            } else if let Some(value) = record.strip_prefix(b"author-mail ") {
                email = String::from_utf8_lossy(value)
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_owned();
            } else if let Some(value) = record.strip_prefix(b"author-time ") {
                author_time = String::from_utf8_lossy(value).parse().ok();
            } else if let Some(value) = record.strip_prefix(b"author-tz ") {
                author_tz = Some(String::from_utf8_lossy(value).into_owned());
            } else if let Some(value) = record.strip_prefix(b"filename ") {
                filename = decode_filename(value)?;
            } else if record == b"boundary" {
                boundary = true;
            }
        }
        Ok(())
    })();
    if truncated || parsed.is_err() {
        stop.store(true, Ordering::SeqCst);
    }
    drop(reader);
    let outcome = watcher.join().map_err(|_| "git blame watcher panicked")??;
    cancel.check()?;
    parsed?;
    if matches!(outcome, ChildOutcome::Deadline)
        || deadline.is_some_and(|time| Instant::now() >= time)
    {
        truncated = true;
    }
    if let ChildOutcome::Exited(status) = outcome {
        if !status.success() && !truncated {
            return Err(format!("git blame exited with {status}"));
        }
    }
    Ok((lines, truncated))
}

pub(crate) fn mapped_ids(
    repo: &Repository,
    config: &Config,
    lines: &[BlamedLine],
) -> Result<BTreeMap<(String, String), String>, String> {
    mapped_identity_pairs(
        repo,
        config,
        lines
            .iter()
            .filter_map(|line| line.author.as_ref())
            .map(|id| (id.name.clone(), id.email.clone())),
    )
}

pub(crate) fn mapped_identity_pairs(
    repo: &Repository,
    config: &Config,
    identities: impl IntoIterator<Item = (String, String)>,
) -> Result<BTreeMap<(String, String), String>, String> {
    let identities: Vec<_> = identities
        .into_iter()
        .filter(|(name, email)| {
            !name.contains(['\n', '\r']) && !email.contains(['\n', '\r', '<', '>'])
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if identities.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(&repo.root)
        .args(["check-mailmap", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git check-mailmap: {e}"))?;
    let mut input = child
        .stdin
        .take()
        .ok_or("git check-mailmap stdin unavailable")?;
    let rows = identities.clone();
    let writer = std::thread::spawn(move || -> Result<(), String> {
        for (name, email) in rows {
            writeln!(input, "{name} <{email}>")
                .map_err(|e| format!("git check-mailmap input: {e}"))?;
        }
        Ok(())
    });
    let output = child
        .wait_with_output()
        .map_err(|e| format!("git check-mailmap: {e}"))?;
    writer
        .join()
        .map_err(|_| "git check-mailmap input writer panicked")??;
    if !output.status.success() {
        return Err(format!(
            "git check-mailmap exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mapped: Vec<_> = output
        .stdout
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    if mapped.len() != identities.len() {
        return Err("git check-mailmap returned an unexpected identity count".into());
    }
    identities
        .into_iter()
        .zip(mapped)
        .map(|(raw, row)| {
            let text = String::from_utf8_lossy(row);
            let start = text.rfind('<').ok_or("git check-mailmap omitted email")?;
            let end = text.rfind('>').ok_or("git check-mailmap omitted email")?;
            if end <= start {
                return Err("git check-mailmap returned invalid email".into());
            }
            let mapped = Identity {
                name: text[..start].trim_end().to_owned(),
                email: text[start + 1..end].to_owned(),
            };
            Ok((raw, crate::config::normalize(&mapped, config).0))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::CancelFlag;
    use std::{
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    #[test]
    fn accepts_sha1_and_sha256_blame_headers() {
        for width in [40, 64] {
            let header = format!("{} 12 1 1", "a".repeat(width));
            assert_eq!(
                parse_header(header.as_bytes()),
                Some(("a".repeat(width), 12, 1, false))
            );
            assert_eq!(
                parse_header(format!("^{header}").as_bytes()),
                Some(("a".repeat(width), 12, 1, true))
            );
        }
        assert_eq!(parse_header(b"short 12 1 1"), None);
    }

    #[test]
    fn decodes_git_quoted_filenames_without_losing_bytes() {
        assert_eq!(decode_filename(b"plain name").unwrap(), b"plain name");
        assert_eq!(
            decode_filename(br#""space\040tab\tline\nquote\"slash\\bad\377""#).unwrap(),
            b"space tab\tline\nquote\"slash\\bad\xff"
        );
        assert!(decode_filename(br#""bad\x""#).is_err());
    }

    #[test]
    fn oversized_record_stops_at_memory_cap() {
        let bytes = vec![b'x'; MAX_RECORD_BYTES + 1];
        let mut reader = BufReader::new(bytes.as_slice());
        let mut record = Vec::new();
        let mut total = 0;
        assert!(matches!(
            read_record(&mut reader, &mut record, &mut total, MAX_OUTPUT_BYTES).unwrap(),
            Record::Limit
        ));
        assert!(record.len() <= MAX_RECORD_BYTES);
    }

    #[test]
    fn line_limit_kills_a_chatty_child_and_keeps_one_line() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let mut command = Command::new("sh");
        command.args(["-c", "echo $$ > \"$1\"; printf '%s 1 1 1\\nauthor A\\nauthor-mail <a@x>\\n\\tfirst\\n%s 2 2 1\\nauthor A\\nauthor-mail <a@x>\\n\\tsecond\\n' \"$2\" \"$2\"; exec sleep 30", "sh"])
            .arg(&pid_file).arg("a".repeat(40));
        let (lines, truncated) = blame_command_with_cancel(
            command,
            1,
            &CancelFlag::default(),
            Some(Instant::now() + Duration::from_secs(2)),
            b"a",
        )
        .unwrap();
        assert_eq!(lines.len(), 1);
        assert!(truncated);
        let pid = std::fs::read_to_string(pid_file).unwrap();
        assert!(!Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success());
    }

    #[test]
    fn cancellation_and_deadline_reap_a_stalled_child() {
        for cancel_first in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let pid_file = dir.path().join("pid");
            let mut command = Command::new("sh");
            command
                .args(["-c", "echo $$ > \"$1\"; exec sleep 30", "sh"])
                .arg(&pid_file);
            let flag = CancelFlag::default();
            let worker_flag = flag.clone();
            let deadline =
                Instant::now() + Duration::from_millis(if cancel_first { 1000 } else { 80 });
            let (tx, rx) = mpsc::channel();
            let worker = thread::spawn(move || {
                tx.send(blame_command_with_cancel(
                    command,
                    10,
                    &worker_flag,
                    Some(deadline),
                    b"a",
                ))
                .unwrap()
            });
            let pid = (0..100)
                .find_map(|_| {
                    let pid = std::fs::read_to_string(&pid_file).ok();
                    if pid.is_none() {
                        thread::sleep(Duration::from_millis(10));
                    }
                    pid
                })
                .expect("fake child did not start");
            if cancel_first {
                flag.cancel();
            }
            let outcome = rx
                .recv_timeout(Duration::from_secs(2))
                .expect("blame did not stop promptly");
            if cancel_first {
                assert_eq!(outcome.unwrap_err(), "cancelled");
            } else {
                assert!(outcome.unwrap().1);
            }
            worker.join().unwrap();
            assert!(!Command::new("kill")
                .args(["-0", pid.trim()])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success());
        }
    }
}
