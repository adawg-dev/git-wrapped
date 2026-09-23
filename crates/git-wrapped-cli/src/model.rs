use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

#[derive(Clone, Debug)]
pub struct FileChange {
    pub path: Vec<u8>,
    pub old_path: Option<Vec<u8>>,
    pub additions: u64,
    pub deletions: u64,
    pub binary: bool,
}

#[derive(Clone, Debug)]
pub struct RawCommit {
    pub sha: String,
    pub parents: Vec<String>,
    pub raw_author: Identity,
    pub mapped_author: Identity,
    pub author_time: String,
    pub committer_time: String,
    pub subject: String,
    pub changes: Vec<FileChange>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CommitSummary {
    pub sha: String,
    pub parents: Vec<String>,
    pub raw_author: Identity,
    pub author_id: String,
    pub author_time: String,
    pub committer_time: String,
    pub subject: String,
    pub files_changed: usize,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ContributorAnalytics {
    pub id: String,
    pub name: String,
    pub commits: u64,
    pub commit_percent: f64,
    pub additions: u64,
    pub deletions: u64,
    pub net: i64,
    pub churn: u64,
    pub files_touched: usize,
    pub directories_touched: usize,
    pub active_days: usize,
    pub tenure_days: i64,
    pub longest_streak: u64,
    pub first_seen_month: String,
    pub returning_after_90_days: u64,
    pub first_contribution: String,
    pub latest_contribution: String,
    pub average_commit_size: f64,
    pub median_commit_size: f64,
    pub largest_commit: u64,
    pub largest_deletion: u64,
    pub commits_by_hour: [u64; 24],
    pub commits_by_weekday: [u64; 7],
    pub commits_by_month: [u64; 12],
}

#[derive(Clone, Debug, Serialize)]
pub struct Activity {
    pub month: String,
    pub commits: u64,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ActivityCell {
    pub date: String,
    pub commits: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Peak {
    pub label: String,
    pub count: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Overlap {
    pub first_id: String,
    pub second_id: String,
    pub weeks: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct WordCount {
    pub word: String,
    pub count: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct TreeSample {
    pub sha: String,
    pub author_date: String,
    pub tracked_files: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct CommitRecord {
    pub sha: String,
    pub author_id: String,
    pub author_time: String,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Clone, Debug)]
pub struct TagDate {
    pub name: String,
    pub target_sha: String,
    pub committer_time: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Insights {
    #[serde(rename = "bus_factor_proxy")]
    pub commit_concentration_50: usize,
    pub longest_streak: u64,
    pub current_streak: u64,
    pub busiest_day: Option<Peak>,
    pub busiest_month: Option<Peak>,
    pub peak_hour: Option<Peak>,
    pub burstiness: Option<f64>,
    pub highest_growth_month: Option<Peak>,
    pub highest_churn_month: Option<Peak>,
    pub first_tag_days: Option<i64>,
    pub release_interval_median_days: Option<f64>,
    pub largest_commit: Option<CommitRecord>,
    pub largest_cleanup: Option<CommitRecord>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FileAnalytics {
    pub path_id: String,
    pub display_path: String,
    pub revisions: u64,
    pub additions: u64,
    pub deletions: u64,
    pub churn: u64,
    pub contributors: usize,
    pub first_change: String,
    pub latest_change: String,
    pub exists_at_head: bool,
    pub rename_from: Vec<String>,
    pub longest_quiet_days: i64,
}

#[derive(Clone, Debug, Serialize)]
/// Historical metrics belong to each changed file's immediate parent directory.
/// Rows exist only for parents with changes; current files count recursively beneath each row.
pub struct DirectoryAnalytics {
    pub path_id: String,
    pub display_path: String,
    pub commits: u64,
    pub churn: u64,
    pub contributors: usize,
    pub current_file_count: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct ExtensionAnalytics {
    pub extension: String,
    pub current_files: usize,
    pub historical_churn: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Award {
    pub slug: String,
    pub title: String,
    pub winner_id: String,
    pub winner: String,
    pub metric: String,
    pub value: String,
    pub explanation: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RepositoryMetadata {
    pub name: String,
    pub selected_since: Option<String>,
    pub selected_until: Option<String>,
    pub selected_authors: Vec<String>,
    pub timezone: String,
    pub include_merges: bool,
    pub first_commit: String,
    pub latest_commit: String,
    pub age_days: i64,
    pub total_commits: u64,
    pub total_contributors: usize,
    pub additions: u64,
    pub deletions: u64,
    pub net_historical_lines: i64,
    pub churn: u64,
    pub tracked_files: usize,
    pub tracked_files_scope: String,
    pub shallow: bool,
}

impl RepositoryMetadata {
    pub fn selection_labels(&self) -> Vec<String> {
        let mut labels = Vec::new();
        let dates = [
            self.selected_since
                .as_ref()
                .map(|date| format!("Since {date}")),
            self.selected_until
                .as_ref()
                .map(|date| format!("Until {date}")),
        ];
        let dates = dates.into_iter().flatten().collect::<Vec<_>>().join(" · ");
        if !dates.is_empty() {
            labels.push(dates);
        }
        if !self.selected_authors.is_empty() {
            labels.push(format!("Authors: {}", self.selected_authors.join(", ")));
        }
        if self.timezone != "commit" {
            labels.push(format!("Timezone: {}", self.timezone));
        }
        if !self.include_merges {
            labels.push("Merges excluded".into());
        }
        labels
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RepositoryAnalytics {
    pub repository: RepositoryMetadata,
    pub contributors: Vec<ContributorAnalytics>,
    pub commits: Vec<CommitSummary>,
    pub activity: Vec<Activity>,
    pub activity_heatmap: Vec<ActivityCell>,
    pub activity_by_week: Vec<ActivityCell>,
    pub newcomers_by_month: Vec<Peak>,
    pub collaboration_overlap: Vec<Overlap>,
    pub subject_words: Vec<WordCount>,
    pub tree_samples: Vec<TreeSample>,
    pub insights: Insights,
    pub files: Vec<FileAnalytics>,
    pub directories: Vec<DirectoryAnalytics>,
    pub extensions: Vec<ExtensionAnalytics>,
    pub awards: Vec<Award>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_raw_identity_and_distinct_line_meanings() {
        let c = CommitSummary {
            sha: "a".repeat(40),
            parents: vec![],
            raw_author: Identity {
                name: "A".into(),
                email: "a@x".into(),
            },
            author_id: "a@x".into(),
            author_time: "2024-01-01T01:00:00+00:00".into(),
            committer_time: "2024-01-01T01:00:00+00:00".into(),
            subject: "first".into(),
            files_changed: 1,
            additions: 3,
            deletions: 1,
        };
        let value = serde_json::to_value(&c).unwrap();
        assert_eq!(value["raw_author"]["email"], "a@x");
        assert_eq!(value["additions"], 3);
        assert_eq!(value["deletions"], 1);
    }
}
