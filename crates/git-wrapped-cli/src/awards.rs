use crate::model::{Award, ContributorAnalytics, RepositoryAnalytics};
use chrono::DateTime;

#[derive(Clone, Copy)]
enum Kind {
    Commits,
    Additions,
    Deletions,
    Net,
    Night,
    Early,
    Weekend,
    LargestCommit,
    LargestDeletion,
    Files,
    Churn,
    Directories,
    Streak,
    Archaeologist,
    Newcomer,
    FileHopper,
    Refactor,
}

const DEFINITIONS: [(&str, &str, &str, &str, Kind); 17] = [
    ("commit-machine", "Commit Machine", "commits", "Most commits authored.", Kind::Commits),
    ("code-creator", "Code Creator", "lines added", "Most lines added.", Kind::Additions),
    ("code-destroyer", "Code Destroyer", "lines deleted", "Most lines deleted.", Kind::Deletions),
    ("net-positive", "Net Positive", "net lines", "Largest positive net line change.", Kind::Net),
    ("night-owl", "Night Owl", "commits at 00–04", "Highest share of commits from 00:00 through 04:59, among contributors with at least five commits.", Kind::Night),
    ("early-bird", "Early Bird", "commits at 05–08", "Highest share of commits from 05:00 through 08:59, among contributors with at least five commits.", Kind::Early),
    ("weekend-warrior", "Weekend Warrior", "weekend commits", "Highest share of commits on Saturday or Sunday, among contributors with at least five commits.", Kind::Weekend),
    ("biggest-bang", "Biggest Bang", "largest commit churn", "Most lines added and deleted in one commit.", Kind::LargestCommit),
    ("biggest-cleanup", "Biggest Cleanup", "largest commit deletion", "Most lines deleted in one commit.", Kind::LargestDeletion),
    ("repo-explorer", "Repo Explorer", "unique files touched", "Most unique files touched.", Kind::Files),
    ("churn-champion", "Churn Champion", "lines changed", "Most historical lines added and deleted.", Kind::Churn),
    ("directory-nomad", "Directory Nomad", "directories touched", "Most distinct directories changed.", Kind::Directories),
    ("streak-keeper", "Streak Keeper", "consecutive active days", "Longest run of consecutive author-local days with a commit.", Kind::Streak),
    ("repo-archaeologist", "Repo Archaeologist", "first contribution", "Earliest first contribution by author time.", Kind::Archaeologist),
    ("newcomer", "Newcomer", "first contribution", "Most recent first contribution by author time.", Kind::Newcomer),
    ("file-hopper", "File Hopper", "unique files touched", "Most distinct files changed across history.", Kind::FileHopper),
    ("refactor-goblin", "Refactor Goblin", "net lines deleted", "Largest positive net deletion across all authored commits.", Kind::Refactor),
];

fn score(contributor: &ContributorAnalytics, kind: Kind) -> Option<(u64, u64)> {
    let count = match kind {
        Kind::Commits => contributor.commits,
        Kind::Additions => contributor.additions,
        Kind::Deletions => contributor.deletions,
        Kind::Net => u64::try_from(contributor.net).unwrap_or(0),
        Kind::Night => contributor.commits_by_hour[..5].iter().sum(),
        Kind::Early => contributor.commits_by_hour[5..9].iter().sum(),
        Kind::Weekend => contributor.commits_by_weekday[5..].iter().sum(),
        Kind::LargestCommit => contributor.largest_commit,
        Kind::LargestDeletion => contributor.largest_deletion,
        Kind::Files => contributor.files_touched as u64,
        Kind::Churn => contributor.churn,
        Kind::Directories => contributor.directories_touched as u64,
        Kind::Streak => contributor.longest_streak,
        Kind::FileHopper => contributor.files_touched as u64,
        Kind::Refactor => u64::try_from(-i128::from(contributor.net)).unwrap_or(0),
        Kind::Archaeologist | Kind::Newcomer => return None,
    };
    if count == 0
        || (matches!(kind, Kind::Night | Kind::Early | Kind::Weekend) && contributor.commits < 5)
    {
        return None;
    }
    let denominator = if matches!(kind, Kind::Night | Kind::Early | Kind::Weekend) {
        contributor.commits
    } else {
        1
    };
    Some((count, denominator))
}

fn award(
    slug: &str,
    title: &str,
    winner: &ContributorAnalytics,
    metric: &str,
    value: String,
    explanation: &str,
) -> Award {
    Award {
        slug: slug.into(),
        title: title.into(),
        winner_id: winner.id.clone(),
        winner: winner.name.clone(),
        metric: metric.into(),
        value,
        explanation: explanation.into(),
    }
}

pub fn select_awards(data: &RepositoryAnalytics) -> Vec<Award> {
    let mut awards: Vec<Award> = DEFINITIONS
        .iter()
        .filter_map(|&(slug, title, metric, explanation, kind)| {
            if matches!(kind, Kind::Archaeologist | Kind::Newcomer) {
                let winner = data
                    .contributors
                    .iter()
                    .filter_map(|c| {
                        DateTime::parse_from_rfc3339(&c.first_contribution)
                            .ok()
                            .map(|time| (c, time))
                    })
                    .max_by(|(a, a_time), (b, b_time)| {
                        let order = a_time.cmp(b_time);
                        (if matches!(kind, Kind::Archaeologist) {
                            order.reverse()
                        } else {
                            order
                        })
                        .then_with(|| b.id.cmp(&a.id))
                    })?
                    .0;
                return Some(award(
                    slug,
                    title,
                    winner,
                    metric,
                    winner.first_contribution.clone(),
                    explanation,
                ));
            }
            let (winner, (count, denominator)) = data
                .contributors
                .iter()
                .filter_map(|contributor| {
                    score(contributor, kind).map(|score| (contributor, score))
                })
                .max_by(|(a, (a_count, a_total)), (b, (b_count, b_total))| {
                    ((*a_count as u128) * (*b_total as u128))
                        .cmp(&((*b_count as u128) * (*a_total as u128)))
                        .then_with(|| b.id.cmp(&a.id))
                })?;
            let value =
                if denominator == 1 && !matches!(kind, Kind::Night | Kind::Early | Kind::Weekend) {
                    count.to_string()
                } else {
                    format!("{:.1}%", 100.0 * count as f64 / denominator as f64)
                };
            Some(award(slug, title, winner, metric, value, explanation))
        })
        .collect();
    for (slug, title, metric, explanation, peak) in [
        (
            "growth-spurt",
            "Growth Spurt",
            "net lines added in month",
            "Largest monthly net line growth",
            data.insights.highest_growth_month.as_ref(),
        ),
        (
            "cleanup-crew",
            "Cleanup Crew",
            "lines changed in month",
            "Most historical lines added and deleted in a month",
            data.insights.highest_churn_month.as_ref(),
        ),
    ] {
        if let Some(peak) = peak.filter(|peak| peak.count > 0) {
            awards.push(Award {
                slug: slug.into(),
                title: title.into(),
                winner_id: "repository".into(),
                winner: data.repository.name.clone(),
                metric: metric.into(),
                value: peak.count.to_string(),
                explanation: format!("{explanation} ({}).", peak.label),
            });
        }
    }
    awards
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Peak, RepositoryMetadata};

    fn contributor(id: &str, commits: u64, night: u64) -> ContributorAnalytics {
        let mut commits_by_hour = [0; 24];
        commits_by_hour[1] = night;
        ContributorAnalytics {
            id: id.into(),
            name: id.into(),
            commits,
            commit_percent: 0.0,
            additions: 0,
            deletions: 0,
            net: 0,
            churn: 0,
            files_touched: 0,
            directories_touched: 0,
            active_days: 0,
            tenure_days: 0,
            longest_streak: 0,
            first_seen_month: String::new(),
            returning_after_90_days: 0,
            first_contribution: String::new(),
            latest_contribution: String::new(),
            average_commit_size: 0.0,
            median_commit_size: 0.0,
            largest_commit: 0,
            largest_deletion: 0,
            commits_by_hour,
            commits_by_weekday: [0; 7],
            commits_by_month: [0; 12],
        }
    }

    fn data(contributors: Vec<ContributorAnalytics>) -> RepositoryAnalytics {
        RepositoryAnalytics {
            repository: RepositoryMetadata {
                name: String::new(),
                selected_since: None,
                selected_until: None,
                selected_authors: vec![],
                timezone: "commit".into(),
                include_merges: true,
                first_commit: String::new(),
                latest_commit: String::new(),
                age_days: 0,
                total_commits: 0,
                total_contributors: 0,
                additions: 0,
                deletions: 0,
                net_historical_lines: 0,
                churn: 0,
                tracked_files: 0,
                tracked_files_scope: "HEAD".into(),
                shallow: false,
            },
            contributors,
            commits: vec![],
            activity: vec![],
            activity_heatmap: vec![],
            activity_by_week: vec![],
            newcomers_by_month: vec![],
            collaboration_overlap: vec![],
            subject_words: vec![],
            tree_samples: vec![],
            insights: Default::default(),
            files: vec![],
            directories: vec![],
            extensions: vec![],
            awards: vec![],
        }
    }

    #[test]
    fn ratio_tie_and_eligibility_ignore_contributor_order() {
        let a = contributor("a", 10, 2);
        let z = contributor("z", 5, 1);
        for contributors in [vec![a.clone(), z.clone()], vec![z.clone(), a.clone()]] {
            let awards = select_awards(&data(contributors));
            let night = awards
                .iter()
                .find(|award| award.slug == "night-owl")
                .unwrap();
            assert_eq!(night.winner_id, "a");
            assert_eq!(night.value, "20.0%");
        }
        assert!(
            select_awards(&data(vec![contributor("z", 4, 4), contributor("a", 5, 0)]))
                .iter()
                .all(|award| award.slug != "night-owl")
        );
    }

    #[test]
    fn extra_awards_have_stable_ties_and_real_values() {
        let mut a = contributor("a", 6, 0);
        let mut z = contributor("z", 6, 0);
        a.churn = 12;
        z.churn = 12;
        let awards = select_awards(&data(vec![z, a]));
        let winner = awards.iter().find(|x| x.slug == "churn-champion").unwrap();
        assert_eq!(winner.winner_id, "a");
        assert_eq!(winner.value, "12");
    }

    #[test]
    fn zero_value_awards_are_omitted() {
        let awards = select_awards(&data(vec![contributor("a", 1, 0)]));
        for slug in [
            "churn-champion",
            "directory-nomad",
            "streak-keeper",
            "file-hopper",
            "refactor-goblin",
            "growth-spurt",
            "cleanup-crew",
            "repo-archaeologist",
            "newcomer",
        ] {
            assert!(!awards.iter().any(|award| award.slug == slug), "{slug}");
        }
    }

    #[test]
    fn contributor_awards_use_real_metrics_and_instant_order() {
        let mut a = contributor("a", 2, 0);
        a.directories_touched = 2;
        a.longest_streak = 3;
        a.first_contribution = "2024-01-01T23:00:00-08:00".into();
        a.files_touched = 6;
        a.net = -5;
        let mut z = contributor("z", 2, 0);
        z.directories_touched = 3;
        z.longest_streak = 4;
        z.first_contribution = "2024-01-02T00:00:00+14:00".into();
        z.files_touched = 2;
        z.net = -1;
        let awards = select_awards(&data(vec![a, z]));
        for (slug, winner_id, value) in [
            ("directory-nomad", "z", "3"),
            ("streak-keeper", "z", "4"),
            ("repo-archaeologist", "z", "2024-01-02T00:00:00+14:00"),
            ("newcomer", "a", "2024-01-01T23:00:00-08:00"),
            ("file-hopper", "a", "6"),
            ("refactor-goblin", "a", "5"),
        ] {
            let award = awards.iter().find(|award| award.slug == slug).unwrap();
            assert_eq!(
                (award.winner_id.as_str(), award.value.as_str()),
                (winner_id, value)
            );
            assert!(!award.metric.is_empty());
            assert!(!award.explanation.is_empty());
        }
    }

    #[test]
    fn first_contribution_award_ties_use_normalized_id() {
        let date = "2024-01-01T00:00:00+00:00";
        let mut z = contributor("z", 1, 0);
        z.first_contribution = date.into();
        let mut a = contributor("a", 1, 0);
        a.first_contribution = date.into();
        let awards = select_awards(&data(vec![z, a]));

        for slug in ["repo-archaeologist", "newcomer"] {
            assert_eq!(
                awards
                    .iter()
                    .find(|award| award.slug == slug)
                    .unwrap()
                    .winner_id,
                "a"
            );
        }
    }

    #[test]
    fn repository_awards_use_positive_monthly_records() {
        let mut data = data(vec![]);
        data.repository.name = "example".into();
        data.insights.highest_growth_month = Some(Peak {
            label: "2024-02".into(),
            count: 8,
        });
        data.insights.highest_churn_month = Some(Peak {
            label: "2024-03".into(),
            count: 12,
        });
        let awards = select_awards(&data);
        for (slug, value, month) in [
            ("growth-spurt", "8", "2024-02"),
            ("cleanup-crew", "12", "2024-03"),
        ] {
            let award = awards.iter().find(|award| award.slug == slug).unwrap();
            assert_eq!(award.winner_id, "repository");
            assert_eq!(award.winner, "example");
            assert_eq!(award.value, value);
            assert!(award.explanation.contains(month));
            assert!(!award.metric.is_empty());
        }
        data.insights.highest_growth_month.as_mut().unwrap().count = 0;
        data.insights.highest_churn_month.as_mut().unwrap().count = 0;
        assert!(select_awards(&data).is_empty());
    }
}
