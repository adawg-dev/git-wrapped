use crate::model::RepositoryAnalytics;
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

#[derive(Debug)]
pub struct AppState {
    pub page: Page,
    pub selected: usize,
    pub running: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            page: Page::default(),
            selected: 0,
            running: true,
        }
    }
}

impl AppState {
    pub fn handle_key(&mut self, key: KeyCode, _data: &RepositoryAnalytics) {
        match key {
            KeyCode::Char('q') => self.running = false,
            KeyCode::Tab | KeyCode::Right => self.open(self.page.next()),
            KeyCode::BackTab | KeyCode::Left => self.open(self.page.previous()),
            _ => {}
        }
    }

    fn open(&mut self, page: Page) {
        self.page = page;
        self.selected = 0;
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
