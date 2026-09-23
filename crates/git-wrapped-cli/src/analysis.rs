use crate::{
    config::{normalize, Config},
    git::{scan, Repository},
    model::{
        Activity, ActivityCell, CommitSummary, ContributorAnalytics, RepositoryAnalytics,
        RepositoryMetadata,
    },
};
use chrono::{DateTime, Datelike, FixedOffset, Timelike};
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

        for change in &raw.changes {
            add(&mut commit_additions, change.additions)?;
            add(&mut commit_deletions, change.deletions)?;
            contributor.files.insert(change.path.clone());
            let directory = change
                .path
                .iter()
                .rposition(|&b| b == b'/')
                .map(|index| change.path[..index].to_vec())
                .unwrap_or_else(|| b".".to_vec());
            contributor.directories.insert(directory);
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
    Ok(RepositoryAnalytics {
        repository: RepositoryMetadata {
            name: repo.name.clone(),
            first_commit: first.to_rfc3339(),
            latest_commit: latest.to_rfc3339(),
            age_days: (latest.date_naive() - first.date_naive()).num_days(),
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
        awards: Vec::new(),
    })
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
