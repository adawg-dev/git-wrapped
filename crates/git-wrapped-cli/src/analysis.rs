use crate::{
    awards::select_awards,
    config::{normalize, Config},
    git::{head_paths, reachable_tag_dates, scan, tree_file_count, Repository},
    model::{
        Activity, ActivityCell, CommitRecord, CommitSummary, ContributorAnalytics,
        DirectoryAnalytics, ExtensionAnalytics, FileAnalytics, Insights, Overlap, Peak,
        RepositoryAnalytics, RepositoryMetadata, TagDate, TreeSample, WordCount,
    },
};
use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, Timelike};
use chrono_tz::Tz;
use globset::{Glob, GlobSetBuilder};
use std::{
    collections::{BTreeMap, HashSet},
    fmt,
    str::FromStr,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TimezoneChoice {
    #[default]
    Commit,
    Utc,
    Local,
    Named(Tz),
}

impl fmt::Display for TimezoneChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Commit => f.write_str("commit"),
            Self::Utc => f.write_str("utc"),
            Self::Local => f.write_str("local"),
            Self::Named(zone) => zone.fmt(f),
        }
    }
}

impl FromStr for TimezoneChoice {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "commit" => Ok(Self::Commit),
            "utc" => Ok(Self::Utc),
            "local" => Ok(Self::Local),
            _ => value.parse::<Tz>().map(Self::Named).map_err(|_| {
                format!("invalid timezone '{value}'; use commit, utc, local, or an IANA zone")
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AnalysisOptions {
    pub since: Option<NaiveDate>,
    pub until: Option<NaiveDate>,
    pub author_ids: Vec<String>,
    pub timezone: TimezoneChoice,
    pub include_merges: bool,
    pub exclusions: Vec<String>,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        Self {
            since: None,
            until: None,
            author_ids: Vec::new(),
            timezone: TimezoneChoice::Commit,
            include_merges: true,
            exclusions: Vec::new(),
        }
    }
}

struct WorkingContributor {
    name: String,
    commits: u64,
    additions: u64,
    deletions: u64,
    sizes: Vec<u64>,
    files: HashSet<Vec<u8>>,
    directories: HashSet<Vec<u8>>,
    days: HashSet<NaiveDate>,
    weeks: HashSet<(i32, u32)>,
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

/// Bounded tree history for the opt-in deep analysis path.
pub fn sample_trees(
    repo: &Repository,
    commits: &[CommitSummary],
) -> Result<Vec<TreeSample>, String> {
    let mut chronological: Vec<_> = commits
        .iter()
        .map(|commit| {
            DateTime::parse_from_rfc3339(&commit.author_time)
                .map(|time| (time, commit))
                .map_err(|e| e.to_string())
        })
        .collect::<Result<_, _>>()?;
    chronological.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.sha.cmp(&b.1.sha)));
    let count = chronological.len().min(24);
    let mut samples = Vec::with_capacity(count);
    for index in 0..count {
        let position = if count == 1 {
            0
        } else {
            index * (chronological.len() - 1) / (count - 1)
        };
        let (time, commit) = chronological[position];
        samples.push(TreeSample {
            sha: commit.sha.clone(),
            author_date: time.date_naive().to_string(),
            tracked_files: tree_file_count(repo, &commit.sha)?,
        });
    }
    Ok(samples)
}

const MAX_BUCKETS: i64 = 20_000;

/// Monthly activity over the selected author-date span, including empty months.
pub fn activity_by_month(data: &RepositoryAnalytics) -> Result<Vec<Activity>, String> {
    let months: BTreeMap<i64, &Activity> = data
        .activity
        .iter()
        .map(|month| {
            let date = NaiveDate::parse_from_str(&format!("{}-01", month.month), "%Y-%m-%d")
                .map_err(|e| e.to_string())?;
            Ok((
                i64::from(date.year()) * 12 + i64::from(date.month0()),
                month,
            ))
        })
        .collect::<Result<_, String>>()?;
    let (Some(&first), Some(&last)) = (months.keys().next(), months.keys().next_back()) else {
        return Ok(Vec::new());
    };
    if last - first + 1 > MAX_BUCKETS {
        return Err("monthly activity exceeds 20,000 buckets; use a coarser granularity".into());
    }
    Ok((first..=last)
        .map(|month| {
            months
                .get(&month)
                .map(|a| (*a).clone())
                .unwrap_or(Activity {
                    month: format!(
                        "{:04}-{:02}",
                        month.div_euclid(12),
                        month.rem_euclid(12) + 1
                    ),
                    commits: 0,
                    additions: 0,
                    deletions: 0,
                })
        })
        .collect())
}

/// Daily activity is generated only on request, so long histories stay sparse in JSON.
pub fn activity_by_day(data: &RepositoryAnalytics) -> Result<Vec<ActivityCell>, String> {
    let days: BTreeMap<NaiveDate, u64> = data
        .activity_heatmap
        .iter()
        .map(|cell| {
            Ok((
                NaiveDate::parse_from_str(&cell.date, "%Y-%m-%d").map_err(|e| e.to_string())?,
                cell.commits,
            ))
        })
        .collect::<Result<_, String>>()?;
    let (Some(&first), Some(&last)) = (days.keys().next(), days.keys().next_back()) else {
        return Ok(Vec::new());
    };
    let count = (last - first).num_days() + 1;
    if count > MAX_BUCKETS {
        return Err("daily activity exceeds 20,000 buckets; use monthly activity".into());
    }
    Ok((0..count)
        .map(|offset| {
            let date = first + Duration::days(offset);
            ActivityCell {
                date: date.to_string(),
                commits: days.get(&date).copied().unwrap_or(0),
            }
        })
        .collect())
}

fn activity_by_week(data: &RepositoryAnalytics) -> Result<Vec<ActivityCell>, String> {
    let mut weeks = BTreeMap::<NaiveDate, u64>::new();
    for cell in &data.activity_heatmap {
        let date = NaiveDate::parse_from_str(&cell.date, "%Y-%m-%d").map_err(|e| e.to_string())?;
        let monday = date - Duration::days(i64::from(date.weekday().num_days_from_monday()));
        add(weeks.entry(monday).or_default(), cell.commits)?;
    }
    let (Some(&first), Some(&last)) = (weeks.keys().next(), weeks.keys().next_back()) else {
        return Ok(Vec::new());
    };
    let count = (last - first).num_weeks() + 1;
    if count > MAX_BUCKETS {
        return Err("weekly activity exceeds 20,000 buckets; use monthly activity".into());
    }
    Ok((0..count)
        .map(|offset| {
            let date = first + Duration::weeks(offset);
            ActivityCell {
                date: date.to_string(),
                commits: weeks.get(&date).copied().unwrap_or(0),
            }
        })
        .collect())
}

pub fn derive_insights(data: &RepositoryAnalytics, tags: &[TagDate]) -> Result<Insights, String> {
    let mut commit_counts: Vec<_> = data.contributors.iter().map(|c| c.commits).collect();
    commit_counts.sort_unstable_by(|a, b| b.cmp(a));
    let mut covered = 0u64;
    let half = data.repository.total_commits.div_ceil(2);
    let commit_concentration_50 = commit_counts
        .into_iter()
        .take_while(|&commits| {
            if covered >= half {
                return false;
            }
            covered = covered.saturating_add(commits);
            true
        })
        .count();
    let mut days: Vec<_> = data
        .activity_heatmap
        .iter()
        .filter(|cell| cell.commits > 0)
        .map(|cell| NaiveDate::parse_from_str(&cell.date, "%Y-%m-%d").map_err(|e| e.to_string()))
        .collect::<Result<_, String>>()?;
    days.sort_unstable();
    days.dedup();
    let (mut longest, mut current, mut previous) = (0u64, 0u64, None);
    for day in &days {
        current = if previous.is_some_and(|prev: NaiveDate| prev.succ_opt() == Some(*day)) {
            current.checked_add(1).ok_or("streak overflow")?
        } else {
            1
        };
        longest = longest.max(current);
        previous = Some(*day);
    }
    let peak = |values: Vec<(String, i64)>| {
        values
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
            .map(|(label, count)| Peak { label, count })
    };
    let to_i64 =
        |value: u64| i64::try_from(value).map_err(|_| "peak count exceeds i64".to_string());
    let busiest_day = peak(
        data.activity_heatmap
            .iter()
            .map(|cell| Ok((cell.date.clone(), to_i64(cell.commits)?)))
            .collect::<Result<_, String>>()?,
    );
    let busiest_month = peak(
        data.activity
            .iter()
            .map(|month| Ok((month.month.clone(), to_i64(month.commits)?)))
            .collect::<Result<_, String>>()?,
    );
    let mut hours = [0u64; 24];
    for contributor in &data.contributors {
        for (total, count) in hours.iter_mut().zip(contributor.commits_by_hour) {
            add(total, count)?;
        }
    }
    let peak_hour = peak(
        hours
            .into_iter()
            .enumerate()
            .map(|(hour, count)| Ok((format!("{hour:02}"), to_i64(count)?)))
            .collect::<Result<_, String>>()?,
    );
    let highest_growth_month = peak(
        data.activity
            .iter()
            .map(|month| {
                Ok((
                    month.month.clone(),
                    net_lines(month.additions, month.deletions)?,
                ))
            })
            .collect::<Result<_, String>>()?,
    );
    let highest_churn_month = peak(
        data.activity
            .iter()
            .map(|month| {
                Ok((
                    month.month.clone(),
                    to_i64(
                        month
                            .additions
                            .checked_add(month.deletions)
                            .ok_or("monthly churn overflow")?,
                    )?,
                ))
            })
            .collect::<Result<_, String>>()?,
    );
    let record = |commit: &CommitSummary| CommitRecord {
        sha: commit.sha.clone(),
        author_id: commit.author_id.clone(),
        author_time: commit.author_time.clone(),
        additions: commit.additions,
        deletions: commit.deletions,
    };
    let largest_commit = data
        .commits
        .iter()
        .map(|c| {
            Ok((
                c.additions
                    .checked_add(c.deletions)
                    .ok_or("commit churn overflow")?,
                c,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?
        .into_iter()
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.sha.cmp(&a.1.sha)))
        .map(|(_, c)| record(c));
    let largest_cleanup = data
        .commits
        .iter()
        .max_by(|a, b| {
            a.deletions
                .cmp(&b.deletions)
                .then_with(|| b.sha.cmp(&a.sha))
        })
        .map(record);
    let mut seen_targets = HashSet::new();
    let mut tag_dates: Vec<_> = tags
        .iter()
        .filter(|tag| seen_targets.insert(tag.target_sha.as_str()))
        .map(|tag| {
            DateTime::parse_from_rfc3339(&tag.committer_time)
                .map(|time| time.date_naive())
                .map_err(|e| e.to_string())
        })
        .collect::<Result<_, String>>()?;
    tag_dates.sort_unstable();
    let first_tag_days = days
        .first()
        .zip(tag_dates.first())
        .map(|(first, tag)| (*tag - *first).num_days());
    let mut intervals: Vec<_> = tag_dates
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).num_days())
        .collect();
    intervals.sort_unstable();
    let release_interval_median_days = if intervals.last().copied().unwrap_or(0) == 0 {
        None
    } else {
        let middle = intervals.len() / 2;
        Some(if intervals.len() % 2 == 1 {
            intervals[middle] as f64
        } else {
            intervals[middle - 1] as f64 / 2.0 + intervals[middle] as f64 / 2.0
        })
    };
    let burstiness = if data.activity_by_week.is_empty() {
        None
    } else {
        let counts: Vec<f64> = data
            .activity_by_week
            .iter()
            .map(|cell| cell.commits as f64)
            .collect();
        let mean = counts.iter().sum::<f64>() / counts.len() as f64;
        (mean > 0.0).then(|| {
            (counts
                .iter()
                .map(|count| (count - mean).powi(2))
                .sum::<f64>()
                / counts.len() as f64)
                .sqrt()
                / mean
        })
    };
    Ok(Insights {
        commit_concentration_50,
        longest_streak: longest,
        current_streak: current,
        busiest_day,
        busiest_month,
        peak_hour,
        burstiness,
        highest_growth_month,
        highest_churn_month,
        first_tag_days,
        release_interval_median_days,
        largest_commit,
        largest_cleanup,
    })
}

pub fn analyze(repo: &Repository, config: &Config) -> Result<RepositoryAnalytics, String> {
    analyze_with_options(repo, config, &AnalysisOptions::default())
}

pub fn analyze_with_options(
    repo: &Repository,
    config: &Config,
    options: &AnalysisOptions,
) -> Result<RepositoryAnalytics, String> {
    if options
        .since
        .zip(options.until)
        .is_some_and(|(since, until)| until < since)
    {
        return Err("until date must be on or after since date".into());
    }
    let selected_authors: Vec<String> = options
        .author_ids
        .iter()
        .map(|id| config.canonical_author_id(id))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let excluded_patterns = if options.exclusions.starts_with(&config.exclude) {
        options.exclusions.clone()
    } else {
        config
            .exclude
            .iter()
            .chain(&options.exclusions)
            .cloned()
            .collect()
    };
    let mut excluded_builder = GlobSetBuilder::new();
    for (index, pattern) in excluded_patterns.iter().enumerate() {
        let source = if index < config.exclude.len() {
            format!("{}: ", repo.root.join(".git-wrapped.json").display())
        } else {
            "--exclude: ".into()
        };
        excluded_builder
            .add(Glob::new(pattern).map_err(|error| {
                format!("{source}invalid exclude pattern {pattern:?}: {error}")
            })?);
    }
    let excluded = excluded_builder
        .build()
        .map_err(|error| format!("invalid exclude patterns: {error}"))?;
    let mut contributors: BTreeMap<String, WorkingContributor> = BTreeMap::new();
    let mut activity: BTreeMap<String, Activity> = BTreeMap::new();
    let mut heatmap: BTreeMap<String, ActivityCell> = BTreeMap::new();
    let mut commits = Vec::new();
    let mut files: BTreeMap<Vec<u8>, WorkingFile> = BTreeMap::new();
    let mut directories: BTreeMap<Vec<u8>, WorkingDirectory> = BTreeMap::new();
    let mut extensions: BTreeMap<String, ExtensionAnalytics> = BTreeMap::new();
    let mut subject_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut first: Option<DateTime<FixedOffset>> = None;
    let mut latest: Option<DateTime<FixedOffset>> = None;
    let mut total_commits = 0;
    let mut additions = 0;
    let mut deletions = 0;
    let mut excluded_changes = 0;

    scan(repo, |raw| {
        let author_time = DateTime::parse_from_rfc3339(&raw.author_time)
            .map_err(|e| format!("invalid author time for {}: {e}", raw.sha))?;
        let (id, name) = normalize(&raw.mapped_author, config);
        if !options.include_merges && raw.parents.len() > 1 {
            return Ok(());
        }
        if !selected_authors.is_empty() && selected_authors.binary_search(&id).is_err() {
            return Ok(());
        }
        let time = match options.timezone {
            TimezoneChoice::Commit => author_time,
            TimezoneChoice::Utc => author_time.with_timezone(&chrono::Utc).fixed_offset(),
            TimezoneChoice::Local => author_time.with_timezone(&chrono::Local).fixed_offset(),
            TimezoneChoice::Named(zone) => author_time.with_timezone(&zone).fixed_offset(),
        };
        let day = time.date_naive();
        if options.since.is_some_and(|since| day < since)
            || options.until.is_some_and(|until| day > until)
        {
            return Ok(());
        }
        let month = time.format("%Y-%m").to_string();
        let date = time.date_naive().to_string();
        // Fixed common-word list keeps topics descriptive and reproducible.
        const STOP_WORDS: &[&str] = &["and", "are", "for", "from", "into", "the", "this", "with"];
        for word in raw.subject.split(|c: char| !c.is_alphanumeric()) {
            if word.chars().count() < 3 {
                continue;
            }
            let word = word.to_lowercase();
            if !STOP_WORDS.contains(&word.as_str()) {
                add(subject_counts.entry(word).or_default(), 1)?;
            }
        }
        let mut commit_additions = 0;
        let mut commit_deletions = 0;
        let mut files_changed = 0;
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
                weeks: HashSet::new(),
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
        contributor.days.insert(time.date_naive());
        let week = time.date_naive().iso_week();
        contributor.weeks.insert((week.year(), week.week()));
        if time < contributor.first {
            contributor.first = time;
        }
        if time > contributor.latest {
            contributor.latest = time;
        }

        let mut commit_directories = HashSet::new();
        for change in &raw.changes {
            // Keep byte paths for IDs; use the same lossy display form only for glob matching.
            if excluded.is_match(String::from_utf8_lossy(&change.path).as_ref()) {
                add(&mut excluded_changes, 1)?;
                continue;
            }
            files_changed += 1;
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
            files_changed,
            additions: commit_additions,
            deletions: commit_deletions,
        });
        Ok(())
    })?;

    let (first, latest) = first
        .zip(latest)
        .ok_or("repository has no selected commits")?;
    let head_paths = head_paths(repo)?
        .into_iter()
        .filter(|path| !excluded.is_match(String::from_utf8_lossy(path).as_ref()))
        .collect::<Vec<_>>();
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
    let mut weeks_by_author = BTreeMap::new();
    let mut newcomers = BTreeMap::<String, u64>::new();
    let mut contributors = contributors
        .into_iter()
        .map(|(id, mut c)| {
            let mut days: Vec<_> = c.days.drain().collect();
            days.sort_unstable();
            let mut streak = 0u64;
            let mut longest_streak = 0u64;
            let mut returning_after_90_days = 0u64;
            for (index, day) in days.iter().enumerate() {
                let gap = index
                    .checked_sub(1)
                    .map(|previous| (*day - days[previous]).num_days());
                streak = if gap == Some(1) { streak + 1 } else { 1 };
                longest_streak = longest_streak.max(streak);
                if gap.is_some_and(|days| days > 90) {
                    returning_after_90_days += 1;
                }
            }
            let first_seen_month = days[0].format("%Y-%m").to_string();
            add(newcomers.entry(first_seen_month.clone()).or_default(), 1)?;
            weeks_by_author.insert(id.clone(), c.weeks);
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
                active_days: days.len(),
                tenure_days: (days[days.len() - 1] - days[0]).num_days(),
                longest_streak,
                first_seen_month,
                returning_after_90_days,
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
    let mut weeks = BTreeMap::<(i32, u32), Vec<&str>>::new();
    for contributor in contributors.iter().take(20) {
        for &week in &weeks_by_author[&contributor.id] {
            weeks.entry(week).or_default().push(&contributor.id);
        }
    }
    let mut overlap = BTreeMap::<(String, String), u64>::new();
    for authors in weeks.values_mut() {
        authors.sort_unstable();
        for (index, first) in authors.iter().enumerate() {
            for second in authors.iter().skip(index + 1) {
                add(
                    overlap
                        .entry(((*first).into(), (*second).into()))
                        .or_default(),
                    1,
                )?;
            }
        }
    }
    let mut collaboration_overlap: Vec<_> = overlap
        .into_iter()
        .map(|((first_id, second_id), weeks)| Overlap {
            first_id,
            second_id,
            weeks,
        })
        .collect();
    collaboration_overlap.sort_by(|a, b| {
        b.weeks
            .cmp(&a.weeks)
            .then_with(|| a.first_id.cmp(&b.first_id))
            .then_with(|| a.second_id.cmp(&b.second_id))
    });
    let mut subject_words: Vec<_> = subject_counts
        .into_iter()
        .filter(|(_, count)| *count >= 3)
        .map(|(word, count)| WordCount { word, count })
        .collect();
    subject_words.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.word.cmp(&b.word)));
    subject_words.truncate(30);
    commits.sort_by(|a, b| a.sha.cmp(&b.sha));
    let mut result = RepositoryAnalytics {
        repository: RepositoryMetadata {
            name: repo.name.clone(),
            selected_since: options.since.map(|date| date.to_string()),
            selected_until: options.until.map(|date| date.to_string()),
            selected_authors,
            excluded_patterns,
            excluded_changes,
            timezone: options.timezone.to_string(),
            include_merges: options.include_merges,
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
            tracked_files: head_paths.len(),
            tracked_files_scope: "HEAD".into(),
            shallow: repo.shallow,
        },
        contributors,
        commits,
        activity: activity.into_values().collect(),
        activity_heatmap: heatmap.into_values().collect(),
        activity_by_week: Vec::new(),
        newcomers_by_month: newcomers
            .into_iter()
            .map(|(label, count)| {
                Ok(Peak {
                    label,
                    count: i64::try_from(count).map_err(|_| "newcomer count exceeds i64")?,
                })
            })
            .collect::<Result<_, String>>()?,
        collaboration_overlap,
        subject_words,
        tree_samples: Vec::new(),
        insights: Insights::default(),
        files,
        directories,
        extensions: extensions.into_values().collect(),
        awards: Vec::new(),
    };
    result.activity_by_week = activity_by_week(&result)?;
    let selected_shas: HashSet<_> = result
        .commits
        .iter()
        .map(|commit| commit.sha.as_str())
        .collect();
    let tags: Vec<_> = reachable_tag_dates(repo)?
        .into_iter()
        .filter(|tag| selected_shas.contains(tag.target_sha.as_str()))
        .collect();
    result.insights = derive_insights(&result, &tags)?;
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
