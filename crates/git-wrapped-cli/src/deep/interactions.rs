use super::blame;
use crate::{
    analysis::path_id,
    config::Config,
    git::{scan_with_cancel, Repository},
    model::{DeepCoverage, FilePair, Interaction, RepositoryAnalytics},
    progress::CancelFlag,
};
use chrono::DateTime;
use globset::GlobSet;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsString,
    os::unix::ffi::OsStringExt,
    process::Command,
    time::Instant,
};

const MAX_DELETION_COMMITS: u64 = 10_000;
const MAX_COUPLING_PATHS: usize = 50;
const COUPLING_FILES: usize = 200;
const MAX_PAIRS: usize = 200;

struct Change {
    path: Vec<u8>,
    old_path: Vec<u8>,
    renamed: bool,
    deletions: u64,
}

/// Old-side line ranges removed by a `--unified=0` diff, as (first line, count).
fn removed_ranges(diff: &[u8]) -> Vec<(u64, u64)> {
    diff.split(|&b| b == b'\n')
        .filter_map(|line| {
            let old = line.strip_prefix(b"@@ -")?;
            let old = &old[..old.iter().position(|&b| b == b' ')?];
            let text = std::str::from_utf8(old).ok()?;
            let (start, count) = text.split_once(',').unwrap_or((text, "1"));
            let (start, count) = (start.parse().ok()?, count.parse().ok()?);
            (count > 0).then_some((start, count))
        })
        .collect()
}

fn removed_line_ranges(
    repo: &Repository,
    parent: &str,
    sha: &str,
    change: &Change,
) -> Result<Vec<(u64, u64)>, String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(&repo.root)
        .args([
            "--literal-pathspecs",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--unified=0",
            if change.renamed {
                "--find-renames"
            } else {
                "--no-renames"
            },
            parent,
            sha,
            "--",
        ])
        .arg(OsString::from_vec(change.old_path.clone()));
    if change.renamed {
        command.arg(OsString::from_vec(change.path.clone()));
    }
    let output = command.output().map_err(|e| format!("git diff: {e}"))?;
    if !output.status.success() {
        return Err(format!("git diff exited with {}", output.status));
    }
    Ok(removed_ranges(&output.stdout))
}

fn sort_pairs(coupling: HashMap<(String, String), u64>) -> Vec<FilePair> {
    let mut pairs: Vec<_> = coupling
        .into_iter()
        .map(
            |((first_path_id, second_path_id), cochange_commits)| FilePair {
                first_path_id,
                second_path_id,
                cochange_commits,
            },
        )
        .collect();
    pairs.sort_by(|a, b| {
        b.cochange_commits
            .cmp(&a.cochange_commits)
            .then_with(|| a.first_path_id.cmp(&b.first_path_id))
            .then_with(|| a.second_path_id.cmp(&b.second_path_id))
    });
    pairs.truncate(MAX_PAIRS);
    pairs
}

/// Deleted-line interactions from first-parent diffs of selected nonmerge commits,
/// plus co-change counts over selected commits with 2–50 changed paths.
#[allow(clippy::too_many_arguments)]
pub(super) fn analyze(
    repo: &Repository,
    config: &Config,
    data: &RepositoryAnalytics,
    excluded: &GlobSet,
    max_lines: u64,
    deadline: Option<Instant>,
    cancel: &CancelFlag,
    coverage: &mut DeepCoverage,
) -> Result<(Vec<Interaction>, Vec<FilePair>), String> {
    let selected: HashMap<&str, _> = data.commits.iter().map(|c| (c.sha.as_str(), c)).collect();
    let mut changes = Vec::new();
    // The scanner clears merge changes, so merges never reach either measure.
    scan_with_cancel(repo, cancel, |raw| {
        if let Some(commit) = selected.get(raw.sha.as_str()) {
            let kept: Vec<_> = raw
                .changes
                .into_iter()
                .filter(|c| !excluded.is_match(String::from_utf8_lossy(&c.path).as_ref()))
                .map(|c| Change {
                    renamed: c.old_path.is_some(),
                    old_path: c.old_path.unwrap_or_else(|| c.path.clone()),
                    path: c.path,
                    deletions: if c.binary { 0 } else { c.deletions },
                })
                .collect();
            changes.push((*commit, kept));
        }
        Ok(())
    })?;

    let mut ranked: Vec<_> = data.files.iter().collect();
    ranked.sort_by(|a, b| {
        b.revisions
            .cmp(&a.revisions)
            .then_with(|| a.path_id.cmp(&b.path_id))
    });
    let top: HashSet<&str> = ranked
        .iter()
        .take(COUPLING_FILES)
        .map(|f| f.path_id.as_str())
        .collect();
    let mut coupling = HashMap::<(String, String), u64>::new();
    for (_, commit_changes) in &changes {
        let mut paths: Vec<_> = commit_changes.iter().map(|c| path_id(&c.path)).collect();
        paths.sort_unstable();
        paths.dedup();
        if paths.len() > MAX_COUPLING_PATHS {
            coverage.coupling_commits_skipped += 1;
            continue;
        }
        if paths.len() < 2 {
            continue;
        }
        coverage.coupling_commits_examined += 1;
        paths.retain(|path| top.contains(path.as_str()));
        for (i, first) in paths.iter().enumerate() {
            for second in &paths[i + 1..] {
                *coupling.entry((first.clone(), second.clone())).or_default() += 1;
            }
        }
    }

    let mut deletion_commits: Vec<_> = changes
        .iter()
        .filter(|(commit, list)| commit.parents.len() == 1 && list.iter().any(|c| c.deletions > 0))
        .map(|(commit, list)| {
            DateTime::parse_from_rfc3339(&commit.author_time)
                .map(|time| (time, *commit, list))
                .map_err(|e| e.to_string())
        })
        .collect::<Result<_, _>>()?;
    // Newest first, so a budget cut keeps the most recent measured history.
    deletion_commits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.sha.cmp(&b.1.sha)));
    let mut counts = BTreeMap::<(String, String), u64>::new();
    for (_, commit, list) in deletion_commits {
        cancel.check()?;
        if coverage.interaction_commits_examined >= MAX_DELETION_COMMITS
            || deadline.is_some_and(|at| Instant::now() >= at)
        {
            coverage.interaction_commits_skipped += 1;
            continue;
        }
        let parent = &commit.parents[0];
        let mut local = BTreeMap::<String, u64>::new();
        let mut complete = true;
        for change in list.iter().filter(|c| c.deletions > 0) {
            let ranges = removed_line_ranges(repo, parent, &commit.sha, change)?;
            if ranges.is_empty() {
                continue;
            }
            let (lines, cut) =
                blame::blame_at(repo, parent, &change.old_path, max_lines, cancel, deadline)?;
            if cut {
                complete = false;
                break;
            }
            let removed: Vec<_> = lines
                .iter()
                .filter(|line| {
                    ranges
                        .iter()
                        .any(|&(start, count)| (start..start + count).contains(&line.final_line))
                })
                .cloned()
                .collect();
            let mapped = blame::mapped_ids(repo, config, &removed)?;
            for line in &removed {
                let id = line
                    .author
                    .as_ref()
                    .and_then(|id| mapped.get(&(id.name.clone(), id.email.clone())))
                    .cloned()
                    .unwrap_or_else(|| "unknown".into());
                *local.entry(id).or_default() += 1;
            }
        }
        if !complete {
            coverage.interaction_commits_skipped += 1;
            continue;
        }
        coverage.interaction_commits_examined += 1;
        for (original, lines) in local {
            *counts
                .entry((commit.author_id.clone(), original))
                .or_default() += lines;
        }
    }
    if coverage.interaction_commits_skipped > 0 {
        coverage.truncated = true;
    }
    let mut interactions: Vec<_> = counts
        .into_iter()
        .map(
            |((deleting_author_id, original_author_id), deleted_lines)| Interaction {
                deleting_author_id,
                original_author_id,
                deleted_lines,
            },
        )
        .collect();
    interactions.sort_by(|a, b| {
        b.deleted_lines
            .cmp(&a.deleted_lines)
            .then_with(|| a.deleting_author_id.cmp(&b.deleting_author_id))
            .then_with(|| a.original_author_id.cmp(&b.original_author_id))
    });
    Ok((interactions, sort_pairs(coupling)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_old_side_hunk_ranges() {
        let diff = b"diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -3 +3 @@ ctx\n-a\n+b\n@@ -10,2 +9,0 @@\n-c\n-d\n@@ -12,0 +11 @@\n+e\n";
        assert_eq!(removed_ranges(diff), [(3, 1), (10, 2)]);
    }
}
