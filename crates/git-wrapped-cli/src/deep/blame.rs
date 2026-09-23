use crate::{config::Config, git::Repository, model::Identity};
use std::{
    collections::BTreeMap,
    io::Write,
    os::unix::ffi::OsStringExt,
    process::{Command, Stdio},
};

#[derive(Clone, Debug)]
#[allow(dead_code)] // Origin and timestamp fields feed the sampled history tasks.
pub(crate) struct BlamedLine {
    pub sha: String,
    pub original_line: u64,
    pub filename: Vec<u8>,
    pub author: Option<Identity>,
    pub author_time: Option<i64>,
    pub author_tz: Option<String>,
}

fn git_output(repo: &Repository, args: &[&str], path: &[u8]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(&repo.root)
        .args(args)
        .arg("--")
        .arg(std::ffi::OsString::from_vec(path.to_vec()))
        .output()
        .map_err(|e| format!("git blame: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git blame exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

pub(crate) fn blame(
    repo: &Repository,
    path: &[u8],
    max_lines: u64,
) -> Result<(Vec<BlamedLine>, bool), String> {
    let bytes = git_output(
        repo,
        &[
            "blame",
            "--line-porcelain",
            "--root",
            "--no-textconv",
            "HEAD",
        ],
        path,
    )?;
    let mut lines = Vec::new();
    let mut sha = String::new();
    let mut original_line = 0;
    let mut name = String::new();
    let mut email = String::new();
    let mut author_time = None;
    let mut author_tz = None;
    let mut filename = path.to_vec();
    let mut boundary = false;
    let mut truncated = false;
    for record in bytes.split(|&b| b == b'\n') {
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
                    filename: filename.clone(),
                    author,
                    author_time,
                    author_tz: author_tz.clone(),
                });
            }
            continue;
        }
        if record.len() >= 41
            && record[40] == b' '
            && record[..40].iter().all(u8::is_ascii_hexdigit)
        {
            sha = String::from_utf8_lossy(&record[..40]).into_owned();
            original_line = String::from_utf8_lossy(&record[41..])
                .split_whitespace()
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            name.clear();
            email.clear();
            author_time = None;
            author_tz = None;
            filename = path.to_vec();
            boundary = false;
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
            filename = value.to_vec();
        } else if record == b"boundary" {
            boundary = true;
        }
    }
    Ok((lines, truncated))
}

pub(crate) fn mapped_ids(
    repo: &Repository,
    config: &Config,
    lines: &[BlamedLine],
) -> Result<BTreeMap<(String, String), String>, String> {
    let identities: Vec<_> = lines
        .iter()
        .filter_map(|line| line.author.as_ref())
        .filter(|id| !id.name.contains(['\n', '\r']) && !id.email.contains(['\n', '\r', '<', '>']))
        .map(|id| (id.name.clone(), id.email.clone()))
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
