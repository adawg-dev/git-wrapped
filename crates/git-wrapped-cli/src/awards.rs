use crate::model::{Award, ContributorAnalytics, RepositoryAnalytics};

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
}

const DEFINITIONS: [(&str, &str, &str, &str, Kind); 10] = [
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
    DEFINITIONS
        .iter()
        .filter_map(|&(slug, title, metric, explanation, kind)| {
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
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RepositoryMetadata;

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
                shallow: false,
            },
            contributors,
            commits: vec![],
            activity: vec![],
            activity_heatmap: vec![],
            activity_by_week: vec![],
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
}
