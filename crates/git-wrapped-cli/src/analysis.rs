use crate::{
    awards::select_awards,
    config::{normalize, Config},
    git::{head_paths, scan, Repository},
    model::{
        Activity, ActivityCell, CommitSummary, ContributorAnalytics, DirectoryAnalytics,
        ExtensionAnalytics, FileAnalytics, RepositoryAnalytics, RepositoryMetadata,
    },
};
use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Timelike};
use std::collections::{BTreeMap, HashSet};

struct WorkingContributor {
    name: String,
    commits: u64,
    additions: u64,
    deletions: u64,
    sizes: Vec<u64>,
    files: HashSet<Vec<u8>>,
    directories: HashSet<Vec<u8>>,
    days: HashSet<String>,
    first: DateTime<FixedOffset>,
    latest: DateTime<FixedOffset>,
    by_hour: [u64; 24],
    by_weekday: [u64; 7],
    by_month: [u64; 12],
    largest_commit: u64,
    largest_deletion: u64,
}

struct WorkingFile {
    revisions: u64,
    additions: u64,
    deletions: u64,
    churn: u64,
    contributors: HashSet<String>,
    dates: Vec<NaiveDate>,
    first: DateTime<FixedOffset>,
    latest: DateTime<FixedOffset>,
    rename_from: Vec<Vec<u8>>,
}

#[derive(Default)]
struct WorkingDirectory {
    commits: u64,
    churn: u64,
    contributors: HashSet<String>,
    current_file_count: usize,
}

fn directory(path: &[u8]) -> Vec<u8> {
    path.iter()
        .rposition(|&byte| byte == b'/')
        .map(|index| path[..index].to_vec())
        .unwrap_or_else(|| b".".to_vec())
}

fn extension(path: &[u8]) -> String {
    let name = path.rsplit(|&byte| byte == b'/').next().unwrap_or(path);
    let suffix = name.iter().rposition(|&byte| byte == b'.');
    match suffix {
        Some(index) if index > 0 && index + 1 < name.len() => {
            String::from_utf8_lossy(&name[index + 1..]).to_lowercase()
        }
        _ => "[none]".into(),
    }
}

fn path_id(path: &[u8]) -> String {
    path.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn add(total: &mut u64, value: u64) -> Result<(), String> {
    *total = total
        .checked_add(value)
        .ok_or("line or commit count overflow")?;
    Ok(())
}

fn net_lines(additions: u64, deletions: u64) -> Result<i64, String> {
    let added = i64::try_from(additions).map_err(|_| "additions exceed i64")?;
    let deleted = i64::try_from(deletions).map_err(|_| "deletions exceed i64")?;
    added
        .checked_sub(deleted)
        .ok_or_else(|| "net lines exceed i64".into())
}

fn median_commit_size(sizes: &mut [u64]) -> f64 {
    if sizes.is_empty() {
        return 0.0;
    }
    sizes.sort_unstable();
    let middle = sizes.len() / 2;
    if sizes.len() % 2 == 1 {
        sizes[middle] as f64
    } else {
        sizes[middle - 1] as f64 / 2.0 + sizes[middle] as f64 / 2.0
    }
}

pub fn analyze(repo: &Repository, config: &Config) -> Result<RepositoryAnalytics, String> {
    let mut contributors: BTreeMap<String, WorkingContributor> = BTreeMap::new();
    let mut activity: BTreeMap<String, Activity> = BTreeMap::new();
    let mut heatmap: BTreeMap<String, ActivityCell> = BTreeMap::new();
    let mut commits = Vec::new();
    let mut files: BTreeMap<Vec<u8>, WorkingFile> = BTreeMap::new();
    let mut directories: BTreeMap<Vec<u8>, WorkingDirectory> = BTreeMap::new();
    let mut extensions: BTreeMap<String, ExtensionAnalytics> = BTreeMap::new();
    let mut first: Option<DateTime<FixedOffset>> = None;
    let mut latest: Option<DateTime<FixedOffset>> = None;
    let mut total_commits = 0;
    let mut additions = 0;
    let mut deletions = 0;

    scan(repo, |raw| {
        let time = DateTime::parse_from_rfc3339(&raw.author_time)
            .map_err(|e| format!("invalid author time for {}: {e}", raw.sha))?;
        let (id, name) = normalize(&raw.mapped_author, config);
        let month = time.format("%Y-%m").to_string();
        let date = time.date_naive().to_string();
        let mut commit_additions = 0;
        let mut commit_deletions = 0;
        let contributor = contributors
            .entry(id.clone())
            .or_insert_with(|| WorkingContributor {
                name: name.clone(),
                commits: 0,
                additions: 0,
                deletions: 0,
                sizes: Vec::new(),
                files: HashSet::new(),
                directories: HashSet::new(),
                days: HashSet::new(),
                first: time,
                latest: time,
                by_hour: [0; 24],
                by_weekday: [0; 7],
                by_month: [0; 12],
                largest_commit: 0,
                largest_deletion: 0,
            });
        // Pick one stable display name if an identity used several names.
        if name < contributor.name {
            contributor.name = name;
        }
        add(&mut contributor.commits, 1)?;
        add(&mut contributor.by_hour[time.hour() as usize], 1)?;
        add(
            &mut contributor.by_weekday[time.weekday().num_days_from_monday() as usize],
            1,
        )?;
        add(&mut contributor.by_month[time.month0() as usize], 1)?;
        contributor.days.insert(date.clone());
        if time < contributor.first {
            contributor.first = time;
        }
        if time > contributor.latest {
            contributor.latest = time;
        }

        let mut commit_directories = HashSet::new();
        for change in &raw.changes {
            add(&mut commit_additions, change.additions)?;
            add(&mut commit_deletions, change.deletions)?;
            contributor.files.insert(change.path.clone());
            let dir = directory(&change.path);
            contributor.directories.insert(dir.clone());
            commit_directories.insert(dir.clone());
            let churn = change
                .additions
                .checked_add(change.deletions)
                .ok_or("file churn overflow")?;
            let file = files
                .entry(change.path.clone())
                .or_insert_with(|| WorkingFile {
                    revisions: 0,
                    additions: 0,
                    deletions: 0,
                    churn: 0,
                    contributors: HashSet::new(),
                    dates: Vec::new(),
                    first: time,
                    latest: time,
                    rename_from: Vec::new(),
                });
            add(&mut file.revisions, 1)?;
            add(&mut file.additions, change.additions)?;
            add(&mut file.deletions, change.deletions)?;
            add(&mut file.churn, churn)?;
            file.contributors.insert(id.clone());
            file.dates.push(time.date_naive());
            file.first = file.first.min(time);
            file.latest = file.latest.max(time);
            if let Some(old_path) = &change.old_path {
                file.rename_from.push(old_path.clone());
            }
            let dir_entry = directories.entry(dir).or_default();
            add(&mut dir_entry.churn, churn)?;
            dir_entry.contributors.insert(id.clone());
            let ext = extension(&change.path);
            let ext_entry = extensions.entry(ext.clone()).or_insert(ExtensionAnalytics {
                extension: ext,
                current_files: 0,
                historical_churn: 0,
            });
            add(&mut ext_entry.historical_churn, churn)?;
        }
        for dir in commit_directories {
            add(&mut directories.get_mut(&dir).unwrap().commits, 1)?;
        }
        let size = commit_additions
            .checked_add(commit_deletions)
            .ok_or("commit churn overflow")?;
        contributor.sizes.push(size);
        add(&mut contributor.additions, commit_additions)?;
        add(&mut contributor.deletions, commit_deletions)?;
        contributor.largest_commit = contributor.largest_commit.max(size);
        contributor.largest_deletion = contributor.largest_deletion.max(commit_deletions);
        add(&mut total_commits, 1)?;
        add(&mut additions, commit_additions)?;
        add(&mut deletions, commit_deletions)?;
        if first.is_none_or(|old| time < old) {
            first = Some(time);
        }
        if latest.is_none_or(|old| time > old) {
            latest = Some(time);
        }

        let period = activity.entry(month.clone()).or_insert(Activity {
            month,
            commits: 0,
            additions: 0,
            deletions: 0,
        });
        add(&mut period.commits, 1)?;
        add(&mut period.additions, commit_additions)?;
        add(&mut period.deletions, commit_deletions)?;
        let cell = heatmap
            .entry(date.clone())
            .or_insert(ActivityCell { date, commits: 0 });
        add(&mut cell.commits, 1)?;
        commits.push(CommitSummary {
            sha: raw.sha,
            parents: raw.parents,
            raw_author: raw.raw_author,
            author_id: id,
            author_time: raw.author_time,
            committer_time: raw.committer_time,
            subject: raw.subject,
            files_changed: raw.changes.len(),
            additions: commit_additions,
            deletions: commit_deletions,
        });
        Ok(())
    })?;

    let (first, latest) = first.zip(latest).ok_or("repository has no commits")?;
    let head_paths = head_paths(repo)?;
    for path in &head_paths {
        let ext = extension(path);
        let ext_entry = extensions.entry(ext.clone()).or_insert(ExtensionAnalytics {
            extension: ext,
            current_files: 0,
            historical_churn: 0,
        });
        ext_entry.current_files = ext_entry
            .current_files
            .checked_add(1)
            .ok_or("extension file count overflow")?;
        let mut dir = directory(path);
        loop {
            if let Some(entry) = directories.get_mut(&dir) {
                entry.current_file_count = entry
                    .current_file_count
                    .checked_add(1)
                    .ok_or("directory file count overflow")?;
            }
            if dir == b"." {
                break;
            }
            dir = directory(&dir);
        }
    }
    let head_set: HashSet<&[u8]> = head_paths.iter().map(Vec::as_slice).collect();
    let files = files
        .into_iter()
        .map(|(path, mut file)| {
            file.dates.sort_unstable();
            let longest_quiet_days = file
                .dates
                .windows(2)
                .map(|pair| (pair[1] - pair[0]).num_days())
                .max()
                .unwrap_or(0);
            FileAnalytics {
                path_id: path_id(&path),
                display_path: String::from_utf8_lossy(&path).into_owned(),
                revisions: file.revisions,
                additions: file.additions,
                deletions: file.deletions,
                churn: file.churn,
                contributors: file.contributors.len(),
                first_change: file.first.to_rfc3339(),
                latest_change: file.latest.to_rfc3339(),
                exists_at_head: head_set.contains(path.as_slice()),
                rename_from: file.rename_from.iter().map(|old| path_id(old)).collect(),
                longest_quiet_days,
            }
        })
        .collect();
    let directories = directories
        .into_iter()
        .map(|(path, dir)| DirectoryAnalytics {
            path_id: path_id(&path),
            display_path: String::from_utf8_lossy(&path).into_owned(),
            commits: dir.commits,
            churn: dir.churn,
            contributors: dir.contributors.len(),
            current_file_count: dir.current_file_count,
        })
        .collect();
    let mut contributors = contributors
        .into_iter()
        .map(|(id, mut c)| {
            let churn = c
                .additions
                .checked_add(c.deletions)
                .ok_or("contributor churn overflow")?;
            Ok(ContributorAnalytics {
                id,
                name: c.name,
                commits: c.commits,
                commit_percent: 100.0 * c.commits as f64 / total_commits as f64,
                additions: c.additions,
                deletions: c.deletions,
                net: net_lines(c.additions, c.deletions)?,
                churn,
                files_touched: c.files.len(),
                directories_touched: c.directories.len(),
                active_days: c.days.len(),
                first_contribution: c.first.to_rfc3339(),
                latest_contribution: c.latest.to_rfc3339(),
                average_commit_size: churn as f64 / c.commits as f64,
                median_commit_size: median_commit_size(&mut c.sizes),
                largest_commit: c.largest_commit,
                largest_deletion: c.largest_deletion,
                commits_by_hour: c.by_hour,
                commits_by_weekday: c.by_weekday,
                commits_by_month: c.by_month,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    contributors.sort_by(|a, b| b.commits.cmp(&a.commits).then_with(|| a.id.cmp(&b.id)));
    commits.sort_by(|a, b| a.sha.cmp(&b.sha));
    let mut result = RepositoryAnalytics {
        repository: RepositoryMetadata {
            name: repo.name.clone(),
            first_commit: first.to_rfc3339(),
            latest_commit: latest.to_rfc3339(),
            age_days: (latest - first).num_days(),
            total_commits,
            total_contributors: contributors.len(),
            additions,
            deletions,
            net_historical_lines: net_lines(additions, deletions)?,
            churn: additions
                .checked_add(deletions)
                .ok_or("repository churn overflow")?,
            tracked_files: repo.tracked_files,
            shallow: repo.shallow,
        },
        contributors,
        commits,
        activity: activity.into_values().collect(),
        activity_heatmap: heatmap.into_values().collect(),
        files,
        directories,
        extensions: extensions.into_values().collect(),
        awards: Vec::new(),
    };
    result.awards = select_awards(&result);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{median_commit_size, net_lines};

    #[test]
    fn checked_net_rejects_values_outside_signed_range() {
        assert_eq!(net_lines(5, 9).unwrap(), -4);
        assert!(net_lines(u64::MAX, 0).is_err());
        assert!(net_lines(0, u64::MAX).is_err());
    }

    #[test]
    fn median_counts_zero_churn_commits() {
        assert_eq!(median_commit_size(&mut [0, 0, 2, 4]), 1.0);
        assert_eq!(median_commit_size(&mut [0, 2, 3]), 2.0);
    }
}
