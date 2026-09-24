use super::{blame, blob, entries_at, DeepLimits};
use crate::{
    analysis::{path_id, TimezoneChoice},
    config::Config,
    git::Repository,
    model::{
        CodeAge, DeepCoverage, OwnershipSlice, OwnershipSnapshot, RepositoryAnalytics,
        SurvivalPoint, YearCohort,
    },
    progress::CancelFlag,
};
use chrono::{DateTime, Datelike, FixedOffset, Utc};
use globset::GlobSet;
use std::{
    collections::{BTreeMap, HashSet},
    time::Instant,
};

type Origin = (String, String, u64); // SHA, byte-preserving path ID, original line

pub(super) struct HistoryBudget {
    pub limits: DeepLimits,
    pub deadline: Option<Instant>,
    pub files: usize,
    pub lines: u64,
}

fn origins(lines: &[blame::BlamedLine]) -> HashSet<Origin> {
    lines
        .iter()
        .map(|line| {
            (
                line.sha.clone(),
                path_id(&line.filename),
                line.original_line,
            )
        })
        .collect()
}

pub(super) fn selected_time(
    time: i64,
    offset: Option<&str>,
    timezone: TimezoneChoice,
) -> Option<DateTime<FixedOffset>> {
    let utc = DateTime::<Utc>::from_timestamp(time, 0)?;
    Some(match timezone {
        TimezoneChoice::Commit => {
            let bytes = offset?.as_bytes();
            if bytes.len() != 5
                || !matches!(bytes[0], b'+' | b'-')
                || !bytes[1..].iter().all(u8::is_ascii_digit)
            {
                return None;
            }
            let hours = std::str::from_utf8(&bytes[1..3])
                .ok()?
                .parse::<i32>()
                .ok()?;
            let minutes = std::str::from_utf8(&bytes[3..5])
                .ok()?
                .parse::<i32>()
                .ok()?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            let seconds = (hours * 60 + minutes) * 60 * if bytes[0] == b'-' { -1 } else { 1 };
            utc.with_timezone(&FixedOffset::east_opt(seconds)?)
        }
        TimezoneChoice::Utc => utc.fixed_offset(),
        TimezoneChoice::Local => utc.with_timezone(&chrono::Local).fixed_offset(),
        TimezoneChoice::Named(zone) => utc.with_timezone(&zone).fixed_offset(),
    })
}

fn code_age(
    data: &RepositoryAnalytics,
    timezone: TimezoneChoice,
    lines: &[blame::BlamedLine],
) -> CodeAge {
    let latest = data
        .commits
        .iter()
        .filter_map(|c| DateTime::parse_from_rfc3339(&c.author_time).ok())
        .map(|t| t.timestamp())
        .max();
    let mut ages = Vec::new();
    let mut years = BTreeMap::<i32, u64>::new();
    let mut future_dated_lines = 0;
    if let Some(latest) = latest {
        for line in lines {
            let Some(origin) = line.author_time else {
                continue;
            };
            let Some(date) = selected_time(origin, line.author_tz.as_deref(), timezone) else {
                continue;
            };
            *years.entry(date.year()).or_default() += 1;
            if origin > latest {
                future_dated_lines += 1;
            }
            ages.push((latest.saturating_sub(origin).max(0)) / 86_400);
        }
    }
    ages.sort_unstable();
    let median_days = if ages.is_empty() {
        None
    } else {
        let middle = ages.len() / 2;
        Some(if ages.len() % 2 == 0 {
            (ages[middle - 1] + ages[middle]) / 2
        } else {
            ages[middle]
        })
    };
    CodeAge {
        median_days,
        oldest_days: ages.last().copied(),
        future_dated_lines,
        year_cohorts: years
            .into_iter()
            .map(|(year, surviving_lines)| YearCohort {
                year,
                surviving_lines,
            })
            .collect(),
        ..Default::default()
    }
}

pub(super) fn analyze(
    repo: &Repository,
    config: &Config,
    data: &RepositoryAnalytics,
    mut budget: HistoryBudget,
    cancel: &CancelFlag,
    excluded: &GlobSet,
    head_lines: &[blame::BlamedLine],
) -> Result<(CodeAge, Vec<SurvivalPoint>, Vec<OwnershipSnapshot>, bool), String> {
    let timezone = data.repository.timezone.parse::<TimezoneChoice>()?;
    let age = code_age(data, timezone, head_lines);
    let head = origins(head_lines);
    let mut commits: Vec<_> = data
        .commits
        .iter()
        .map(|commit| {
            DateTime::parse_from_rfc3339(&commit.author_time)
                .map(|time| (time, commit))
                .map_err(|e| e.to_string())
        })
        .collect::<Result<_, _>>()?;
    commits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.sha.cmp(&b.1.sha)));
    let count = commits.len().min(12);
    let mut points = Vec::with_capacity(count);
    let mut ownership = Vec::with_capacity(count);
    let mut truncated = false;
    for index in 0..count {
        cancel.check()?;
        if budget.deadline.is_some_and(|at| Instant::now() >= at)
            || budget.files >= budget.limits.max_files
            || budget.lines >= budget.limits.max_lines
        {
            truncated = true;
            break;
        }
        let position = if count == 1 {
            0
        } else {
            index * (commits.len() - 1) / (count - 1)
        };
        let (instant, commit) = commits[position];
        let date = match timezone {
            TimezoneChoice::Commit => instant,
            TimezoneChoice::Utc => instant.with_timezone(&Utc).fixed_offset(),
            TimezoneChoice::Local => instant.with_timezone(&chrono::Local).fixed_offset(),
            TimezoneChoice::Named(zone) => instant.with_timezone(&zone).fixed_offset(),
        };
        let tree: Vec<_> = entries_at(repo, &commit.sha)?
            .into_iter()
            .filter(|(_, _, path)| !excluded.is_match(String::from_utf8_lossy(path).as_ref()))
            .collect();
        let eligible_files = tree
            .iter()
            .filter(|(mode, _, _)| mode == b"100644" || mode == b"100755")
            .count() as u64;
        let mut snapshot = HashSet::new();
        let mut counts = BTreeMap::<String, u64>::new();
        let mut raw_counts = BTreeMap::<(String, String), u64>::new();
        let mut coverage = DeepCoverage {
            eligible_files,
            skipped_submodules: tree.iter().filter(|(mode, _, _)| mode == b"160000").count() as u64,
            ..Default::default()
        };
        let mut sampled_files = 0;
        let mut partial = false;
        for (mode, oid, path) in tree {
            cancel.check()?;
            if mode != b"100644" && mode != b"100755" {
                continue;
            }
            if budget.deadline.is_some_and(|at| Instant::now() >= at)
                || budget.files >= budget.limits.max_files
                || budget.lines >= budget.limits.max_lines
            {
                partial = true;
                break;
            }
            let Some(content) = blob(repo, &oid)? else {
                partial = true;
                break;
            };
            if content.contains(&0) {
                coverage.skipped_binary += 1;
                continue;
            }
            budget.files += 1;
            sampled_files += 1;
            coverage.analyzed_files += 1;
            if content.is_empty() {
                continue;
            }
            let (blamed, cut) = blame::blame_at(
                repo,
                &commit.sha,
                &path,
                budget.limits.max_lines - budget.lines,
                cancel,
                budget.deadline,
            )?;
            budget.lines += blamed.len() as u64;
            snapshot.extend(origins(&blamed));
            for line in &blamed {
                if let Some(author) = &line.author {
                    *raw_counts
                        .entry((author.name.clone(), author.email.clone()))
                        .or_default() += 1;
                } else {
                    coverage.unknown_lines += 1;
                }
            }
            if cut {
                partial = true;
                break;
            }
        }
        cancel.check()?;
        let mapped = if budget.deadline.is_some_and(|at| Instant::now() >= at) {
            partial = true;
            BTreeMap::new()
        } else {
            blame::mapped_identity_pairs(repo, config, raw_counts.keys().cloned())?
        };
        for (raw, lines) in raw_counts {
            if let Some(id) = mapped.get(&raw) {
                coverage.attributed_lines += lines;
                *counts.entry(id.clone()).or_default() += lines;
            } else {
                coverage.unknown_lines += lines;
            }
        }
        if coverage.unknown_lines > 0 {
            *counts.entry("unknown".into()).or_default() += coverage.unknown_lines;
        }
        let original_lines = snapshot.len() as u64;
        let surviving_lines = snapshot.intersection(&head).count() as u64;
        points.push(SurvivalPoint {
            snapshot_sha: commit.sha.clone(),
            snapshot_date: date.date_naive().to_string(),
            original_lines,
            surviving_lines,
            percent: if original_lines == 0 {
                0.0
            } else {
                100.0 * surviving_lines as f64 / original_lines as f64
            },
            sampled_files,
            eligible_files,
            truncated: partial,
        });
        coverage.truncated = partial;
        let total_lines = coverage.attributed_lines + coverage.unknown_lines;
        ownership.push(OwnershipSnapshot {
            sha: commit.sha.clone(),
            author_date: date.date_naive().to_string(),
            total_lines,
            by_author: counts
                .into_iter()
                .map(|(author_id, lines)| OwnershipSlice {
                    author_id,
                    lines,
                    percent: if total_lines == 0 {
                        0.0
                    } else {
                        100.0 * lines as f64 / total_lines as f64
                    },
                })
                .collect(),
            coverage,
        });
        if partial {
            truncated = true;
            break;
        }
    }
    Ok((age, points, ownership, truncated))
}
