mod blame;
mod history;

type TreeEntry = (Vec<u8>, Vec<u8>, Vec<u8>); // mode, object ID, raw path

use crate::{
    analysis::{sample_trees, TimezoneChoice},
    config::Config,
    git::Repository,
    model::{DeepAnalytics, OwnershipGroup, OwnershipSlice, RepositoryAnalytics},
    progress::CancelFlag,
};
use globset::{Glob, GlobSetBuilder};
use std::{
    collections::BTreeMap,
    process::Command,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub struct DeepLimits {
    pub max_files: usize,
    pub max_lines: u64,
    pub max_seconds: u64,
}

impl Default for DeepLimits {
    fn default() -> Self {
        Self {
            max_files: 20_000,
            max_lines: 2_000_000,
            max_seconds: 300,
        }
    }
}

pub(super) fn git_bytes(repo: &Repository, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(&repo.root)
        .args(args)
        .output()
        .map_err(|e| format!("git {}: {e}", args.first().unwrap_or(&"")))?;
    if !output.status.success() {
        return Err(format!(
            "git {} exited with {}: {}",
            args[0],
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

pub(super) fn entries_at(repo: &Repository, revision: &str) -> Result<Vec<TreeEntry>, String> {
    if revision != "HEAD" && !valid_sha(revision) {
        return Err("invalid tree revision".into());
    }
    let bytes = git_bytes(repo, &["ls-tree", "-r", "-z", revision])?;
    bytes
        .split(|&b| b == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let tab = entry
                .iter()
                .position(|&b| b == b'\t')
                .ok_or("malformed ls-tree entry")?;
            let mut fields = entry[..tab].split(|&b| b == b' ');
            let mode = fields.next().ok_or("missing tree mode")?.to_vec();
            let kind = fields.next().ok_or("missing tree type")?;
            let oid = fields.next().ok_or("missing tree oid")?.to_vec();
            if fields.next().is_some() {
                return Err("malformed ls-tree metadata".into());
            }
            if kind != b"blob" && kind != b"commit" {
                return Err("unexpected ls-tree entry type".into());
            }
            Ok((mode, oid, entry[tab + 1..].to_vec()))
        })
        .collect()
}

fn valid_sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn blob(repo: &Repository, oid: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let oid = String::from_utf8(oid.to_vec()).map_err(|e| e.to_string())?;
    let size = git_bytes(repo, &["cat-file", "-s", &oid])?;
    let size: u64 = String::from_utf8_lossy(&size)
        .trim()
        .parse()
        .map_err(|_| "invalid blob size")?;
    // ponytail: per-blob read cap; stream blobs if a larger deep budget is required.
    if size > 64 * 1024 * 1024 {
        return Ok(None);
    }
    git_bytes(repo, &["cat-file", "blob", &oid]).map(Some)
}

fn directory(path: &[u8]) -> Vec<u8> {
    path.iter()
        .rposition(|&b| b == b'/')
        .map(|i| path[..i].to_vec())
        .unwrap_or_else(|| b".".to_vec())
}

fn extension(path: &[u8]) -> String {
    let name = path.rsplit(|&b| b == b'/').next().unwrap_or(path);
    match name.iter().rposition(|&b| b == b'.') {
        Some(i) if i > 0 && i + 1 < name.len() => {
            String::from_utf8_lossy(&name[i + 1..]).to_lowercase()
        }
        _ => "[none]".into(),
    }
}

fn slices(counts: BTreeMap<String, u64>) -> Vec<OwnershipSlice> {
    let total: u64 = counts.values().sum();
    let mut result: Vec<_> = counts
        .into_iter()
        .map(|(author_id, lines)| OwnershipSlice {
            author_id,
            lines,
            percent: if total == 0 {
                0.0
            } else {
                100.0 * lines as f64 / total as f64
            },
        })
        .collect();
    result.sort_by(|a, b| {
        b.lines
            .cmp(&a.lines)
            .then_with(|| a.author_id.cmp(&b.author_id))
    });
    result
}

fn groups(groups: BTreeMap<Vec<u8>, BTreeMap<String, u64>>) -> Vec<OwnershipGroup> {
    groups
        .into_iter()
        .map(|(path, counts)| {
            let lines = counts.values().sum();
            OwnershipGroup {
                group: String::from_utf8_lossy(&path).into_owned(),
                lines,
                by_author: slices(counts),
            }
        })
        .collect()
}

pub fn analyze_deep(
    repo: &Repository,
    config: &Config,
    data: &mut RepositoryAnalytics,
    limits: DeepLimits,
) -> Result<(), String> {
    analyze_deep_with_cancel(repo, config, data, limits, &CancelFlag::default())
}

pub fn analyze_deep_with_cancel(
    repo: &Repository,
    config: &Config,
    data: &mut RepositoryAnalytics,
    limits: DeepLimits,
    cancel: &CancelFlag,
) -> Result<(), String> {
    cancel.check()?;
    let start = Instant::now();
    let deadline = Duration::from_secs(limits.max_seconds);
    let mut excluded = GlobSetBuilder::new();
    for pattern in &data.repository.excluded_patterns {
        excluded.add(
            Glob::new(pattern).map_err(|e| format!("invalid exclude pattern {pattern:?}: {e}"))?,
        );
    }
    let excluded = excluded
        .build()
        .map_err(|e| format!("invalid exclude patterns: {e}"))?;
    let mut deep = DeepAnalytics::default();
    let mut by_author = BTreeMap::<String, u64>::new();
    let mut by_directory = BTreeMap::<Vec<u8>, BTreeMap<String, u64>>::new();
    let mut by_extension = BTreeMap::<Vec<u8>, BTreeMap<String, u64>>::new();
    let entries: Vec<_> = entries_at(repo, "HEAD")?
        .into_iter()
        .filter(|(_, _, path)| !excluded.is_match(String::from_utf8_lossy(path).as_ref()))
        .collect();
    deep.coverage.eligible_files = entries
        .iter()
        .filter(|(mode, _, _)| mode == b"100644" || mode == b"100755")
        .count() as u64;
    deep.coverage.skipped_submodules = entries
        .iter()
        .filter(|(mode, _, _)| mode == b"160000")
        .count() as u64;
    let mut counted_lines = 0_u64;
    let mut head_lines = Vec::new();
    for (mode, oid, path) in entries {
        cancel.check()?;
        if start.elapsed() >= deadline {
            deep.coverage.truncated = true;
            break;
        }
        if mode == b"160000" {
            continue;
        }
        if mode != b"100644" && mode != b"100755" {
            continue;
        }
        if deep.coverage.analyzed_files as usize >= limits.max_files
            || counted_lines >= limits.max_lines
        {
            deep.coverage.truncated = true;
            break;
        }
        let Some(content) = blob(repo, &oid)? else {
            deep.coverage.truncated = true;
            break;
        };
        cancel.check()?;
        if start.elapsed() >= deadline {
            deep.coverage.truncated = true;
            break;
        }
        if content.contains(&0) {
            deep.coverage.skipped_binary += 1;
            continue;
        }
        deep.coverage.analyzed_files += 1;
        if content.is_empty() {
            continue;
        }
        let (lines, truncated) = blame::blame(
            repo,
            &path,
            limits.max_lines - counted_lines,
            cancel,
            start.checked_add(deadline),
        )?;
        let mapped = blame::mapped_ids(repo, config, &lines)?;
        counted_lines += lines.len() as u64;
        for line in &lines {
            let id = line
                .author
                .as_ref()
                .and_then(|id| mapped.get(&(id.name.clone(), id.email.clone())))
                .cloned()
                .unwrap_or_else(|| "unknown".into());
            if id == "unknown" {
                deep.coverage.unknown_lines += 1;
            } else {
                deep.coverage.attributed_lines += 1;
            }
            *by_author.entry(id.clone()).or_default() += 1;
            *by_directory
                .entry(directory(&path))
                .or_default()
                .entry(id.clone())
                .or_default() += 1;
            *by_extension
                .entry(extension(&path).into_bytes())
                .or_default()
                .entry(id)
                .or_default() += 1;
        }
        head_lines.extend(lines);
        if truncated {
            deep.coverage.truncated = true;
            break;
        }
        cancel.check()?;
        if start.elapsed() >= deadline {
            deep.coverage.truncated = true;
            break;
        }
    }
    deep.ownership = slices(by_author);
    deep.surviving_loc = counted_lines;
    deep.ownership_by_directory = groups(by_directory);
    deep.ownership_by_extension = groups(by_extension);
    let (code_age, survival, historical_ownership, truncated_history) = history::analyze(
        repo,
        config,
        data,
        history::HistoryBudget {
            limits,
            deadline: start.checked_add(deadline),
            files: deep.coverage.analyzed_files as usize,
            lines: counted_lines,
        },
        cancel,
        &excluded,
        &head_lines,
    )?;
    deep.code_age = code_age;
    deep.survival = survival;
    deep.historical_ownership = historical_ownership;
    deep.coverage.truncated |= truncated_history;
    cancel.check()?;
    if start.elapsed() >= deadline {
        deep.coverage.truncated = true;
    }
    let samples = if start.elapsed() >= deadline {
        Vec::new()
    } else {
        let timezone = data.repository.timezone.parse::<TimezoneChoice>()?;
        sample_trees(
            repo,
            &data.commits,
            timezone,
            &data.repository.excluded_patterns,
            cancel,
            start.checked_add(deadline),
        )?
    };
    if start.elapsed() >= deadline {
        deep.coverage.truncated = true;
    }
    cancel.check()?;
    data.tree_samples = samples;
    data.deep = Some(deep);
    Ok(())
}
