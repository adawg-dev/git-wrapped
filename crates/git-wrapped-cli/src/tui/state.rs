use crate::model::{ContributorAnalytics, FileAnalytics, RepositoryAnalytics};
use ratatui::crossterm::event::KeyCode;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Page {
    #[default]
    Overview,
    Contributors,
    Activity,
    Files,
    Awards,
    Ownership,
    Survival,
    Interactions,
}

impl Page {
    pub const ALL: [Page; 8] = [
        Page::Overview,
        Page::Contributors,
        Page::Activity,
        Page::Files,
        Page::Awards,
        Page::Ownership,
        Page::Survival,
        Page::Interactions,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::Overview => "Overview",
            Page::Contributors => "Contributors",
            Page::Activity => "Activity",
            Page::Files => "Files",
            Page::Awards => "Awards",
            Page::Ownership => "Ownership",
            Page::Survival => "Survival",
            Page::Interactions => "Interactions",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|&page| page == self).unwrap_or(0)
    }

    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    pub fn previous(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SortKey {
    #[default]
    Commits,
    Additions,
    Deletions,
    Churn,
    ActiveDays,
    Files,
}

impl SortKey {
    pub fn next(self) -> Self {
        match self {
            Self::Commits => Self::Additions,
            Self::Additions => Self::Deletions,
            Self::Deletions => Self::Churn,
            Self::Churn => Self::ActiveDays,
            Self::ActiveDays => Self::Files,
            Self::Files => Self::Commits,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Commits => "Commits",
            Self::Additions => "Additions",
            Self::Deletions => "Deletions",
            Self::Churn => "Churn",
            Self::ActiveDays => "Active days",
            Self::Files => "Files",
        }
    }

    pub fn value(self, c: &ContributorAnalytics) -> u64 {
        match self {
            Self::Commits => c.commits,
            Self::Additions => c.additions,
            Self::Deletions => c.deletions,
            Self::Churn => c.churn,
            Self::ActiveDays => c.active_days as u64,
            Self::Files => c.files_touched as u64,
        }
    }
}

/// Contributors by descending sort value; ties keep a stable ID order.
pub fn sorted_contributors(
    data: &RepositoryAnalytics,
    sort: SortKey,
) -> Vec<&ContributorAnalytics> {
    let mut rows: Vec<_> = data.contributors.iter().collect();
    rows.sort_by(|a, b| {
        sort.value(b)
            .cmp(&sort.value(a))
            .then_with(|| a.id.cmp(&b.id))
    });
    rows
}

/// Files by descending historical churn, matching the `archaeology` view.
pub fn sorted_files(data: &RepositoryAnalytics) -> Vec<&FileAnalytics> {
    let mut rows: Vec<_> = data.files.iter().collect();
    rows.sort_by(|a, b| {
        b.churn
            .cmp(&a.churn)
            .then_with(|| a.path_id.cmp(&b.path_id))
    });
    rows
}

#[derive(Debug)]
pub struct AppState {
    pub page: Page,
    pub selected: usize,
    pub sort: SortKey,
    pub selected_id: Option<String>,
    pub detail: bool,
    pub running: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            page: Page::default(),
            selected: 0,
            sort: SortKey::default(),
            selected_id: None,
            detail: false,
            running: true,
        }
    }
}

impl AppState {
    pub fn handle_key(&mut self, key: KeyCode, data: &RepositoryAnalytics) {
        let rows_len = self.rows_len(data);
        match key {
            KeyCode::Char('q') => self.running = false,
            KeyCode::Tab | KeyCode::Right => self.open(self.page.next(), data),
            KeyCode::BackTab | KeyCode::Left => self.open(self.page.previous(), data),
            KeyCode::Esc => self.detail = false,
            KeyCode::Enter => self.detail = self.page == Page::Contributors && rows_len > 0,
            KeyCode::Char('s') if self.page == Page::Contributors => {
                self.sort = self.sort.next();
                self.select_contributor(data);
            }
            KeyCode::Down => {
                self.selected = self
                    .selected
                    .saturating_add(1)
                    .min(rows_len.saturating_sub(1));
                self.remember_contributor(data);
            }
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.remember_contributor(data);
            }
            _ => {}
        }
    }

    fn rows_len(&self, data: &RepositoryAnalytics) -> usize {
        match self.page {
            Page::Contributors => data.contributors.len(),
            Page::Files => data.files.len(),
            Page::Awards => data.awards.len(),
            Page::Ownership => data.deep.as_ref().map_or(0, |d| d.ownership.len()),
            Page::Survival => data.deep.as_ref().map_or(0, |d| d.survival.len()),
            Page::Interactions => data.deep.as_ref().map_or(0, |d| d.interactions.len()),
            Page::Overview | Page::Activity => 0,
        }
    }

    fn open(&mut self, page: Page, data: &RepositoryAnalytics) {
        self.page = page;
        self.detail = false;
        self.selected = 0;
        if page == Page::Contributors {
            self.select_contributor(data);
        }
    }

    /// Point `selected` at the remembered contributor in the current sort order.
    fn select_contributor(&mut self, data: &RepositoryAnalytics) {
        let rows = sorted_contributors(data, self.sort);
        self.selected = self
            .selected_id
            .as_ref()
            .and_then(|id| rows.iter().position(|c| &c.id == id))
            .unwrap_or(0);
    }

    fn remember_contributor(&mut self, data: &RepositoryAnalytics) {
        if self.page == Page::Contributors {
            self.selected_id = sorted_contributors(data, self.sort)
                .get(self.selected)
                .map(|c| c.id.clone());
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::{Insights, RepositoryMetadata};

    pub(crate) fn empty_data() -> RepositoryAnalytics {
        RepositoryAnalytics {
            repository: RepositoryMetadata {
                name: "empty".into(),
                selected_since: None,
                selected_until: None,
                selected_authors: vec![],
                excluded_patterns: vec![],
                excluded_changes: 0,
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
            contributors: vec![],
            commits: vec![],
            activity: vec![],
            activity_heatmap: vec![],
            activity_by_week: vec![],
            newcomers_by_month: vec![],
            collaboration_overlap: vec![],
            subject_words: vec![],
            tree_samples: vec![],
            insights: Insights::default(),
            files: vec![],
            directories: vec![],
            extensions: vec![],
            awards: vec![],
            deep: None,
        }
    }

    pub(crate) fn contributor(id: &str, commits: u64, additions: u64) -> ContributorAnalytics {
        ContributorAnalytics {
            id: id.into(),
            name: id.into(),
            commits,
            commit_percent: 0.0,
            additions,
            deletions: 0,
            net: additions as i64,
            churn: additions,
            files_touched: 1,
            directories_touched: 1,
            active_days: 1,
            tenure_days: 0,
            longest_streak: 1,
            first_seen_month: "2024-01".into(),
            returning_after_90_days: 0,
            first_contribution: "2024-01-01".into(),
            latest_contribution: "2024-01-01".into(),
            average_commit_size: 0.0,
            median_commit_size: 0.0,
            largest_commit: 0,
            largest_deletion: 0,
            commits_by_hour: [0; 24],
            commits_by_weekday: [0; 7],
            commits_by_month: [0; 12],
        }
    }

    pub(crate) fn two_contributor_data() -> RepositoryAnalytics {
        let mut data = empty_data();
        data.contributors = vec![contributor("a@x", 5, 1), contributor("b@x", 1, 10)];
        data
    }

    #[test]
    fn sorting_preserves_selected_identity() {
        let data = two_contributor_data();
        let mut state = AppState {
            page: Page::Contributors,
            selected: 1,
            selected_id: Some(data.contributors[1].id.clone()),
            ..AppState::default()
        };
        state.handle_key(KeyCode::Char('s'), &data);
        assert_eq!(state.sort, SortKey::Additions);
        assert_eq!(
            state.selected_id.as_deref(),
            Some(data.contributors[1].id.as_str())
        );
        assert_eq!(
            sorted_contributors(&data, state.sort)[state.selected].id,
            "b@x"
        );
    }

    #[test]
    fn selection_moves_within_rows_and_detail_opens_and_closes() {
        let data = two_contributor_data();
        let mut state = AppState::default();
        state.handle_key(KeyCode::Tab, &data);
        for _ in 0..3 {
            state.handle_key(KeyCode::Down, &data);
        }
        assert_eq!(state.selected, 1);
        assert_eq!(state.selected_id.as_deref(), Some("b@x"));
        state.handle_key(KeyCode::Enter, &data);
        assert!(state.detail);
        state.handle_key(KeyCode::Esc, &data);
        assert!(!state.detail);
        state.handle_key(KeyCode::Tab, &data);
        assert_eq!(state.selected, 0);
        state.handle_key(KeyCode::BackTab, &data);
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn empty_pages_ignore_movement_and_detail() {
        let data = empty_data();
        let mut state = AppState {
            page: Page::Contributors,
            ..AppState::default()
        };
        for key in [
            KeyCode::Down,
            KeyCode::Up,
            KeyCode::Enter,
            KeyCode::Char('s'),
        ] {
            state.handle_key(key, &data);
        }
        assert_eq!(state.selected, 0);
        assert!(!state.detail);
    }

    #[test]
    fn deep_pages_select_within_their_rows() {
        let mut data = empty_data();
        data.deep = Some(crate::model::DeepAnalytics {
            ownership: vec![Default::default(), Default::default()],
            ..Default::default()
        });
        let mut state = AppState {
            page: Page::Ownership,
            ..AppState::default()
        };
        for _ in 0..3 {
            state.handle_key(KeyCode::Down, &data);
        }
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn q_exits_from_any_page() {
        for page in Page::ALL {
            let mut state = AppState {
                page,
                ..AppState::default()
            };
            state.handle_key(KeyCode::Char('q'), &empty_data());
            assert!(!state.running);
        }
    }

    #[test]
    fn tab_cycles_every_page_and_back_tab_reverses() {
        let data = empty_data();
        let mut state = AppState::default();
        for expected in Page::ALL.iter().skip(1).chain(Page::ALL.iter().take(1)) {
            state.handle_key(KeyCode::Tab, &data);
            assert_eq!(state.page, *expected);
        }
        state.handle_key(KeyCode::BackTab, &data);
        assert_eq!(state.page, Page::Interactions);
    }
}
