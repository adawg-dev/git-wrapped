use super::state::{AppState, Page};
use crate::model::RepositoryAnalytics;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, Paragraph, Sparkline, Wrap},
    Frame,
};

pub const MIN_WIDTH: u16 = 40;
pub const MIN_HEIGHT: u16 = 12;

/// Replace control characters so repository text cannot emit terminal escapes.
fn safe(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn plural(count: u64, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

fn section(title: &str) -> Block<'_> {
    Block::new()
        .borders(Borders::TOP)
        .title(title)
        .title_style(Style::new().add_modifier(Modifier::BOLD))
}

pub fn draw(frame: &mut Frame, state: &AppState, data: &RepositoryAnalytics) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new(format!(
                "Resize the terminal to at least {MIN_WIDTH}x{MIN_HEIGHT}"
            ))
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    let index = Page::ALL.iter().position(|&p| p == state.page).unwrap_or(0) + 1;
    frame.render_widget(
        Paragraph::new(format!(
            "Git Wrapped · {} · {} ({index}/{})",
            safe(&data.repository.name),
            state.page.title(),
            Page::ALL.len()
        ))
        .style(Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED)),
        header,
    );
    frame.render_widget(Paragraph::new("Tab/←→ page  q quit"), footer);
    match state.page {
        Page::Overview => overview(frame, body, data),
        page => frame.render_widget(Paragraph::new(page.title()), body),
    }
}

fn overview(frame: &mut Frame, area: Rect, data: &RepositoryAnalytics) {
    let repo = &data.repository;
    let insights = &data.insights;
    let mut stats = vec![
        Line::from(format!(
            "{} · {}",
            plural(repo.total_commits, "commit"),
            plural(repo.total_contributors as u64, "contributor")
        )),
        Line::from(format!(
            "+{} −{} · net {} historical lines",
            repo.additions, repo.deletions, repo.net_historical_lines
        )),
        Line::from(format!(
            "{} → {}",
            safe(repo.first_commit.get(..10).unwrap_or(&repo.first_commit)),
            safe(repo.latest_commit.get(..10).unwrap_or(&repo.latest_commit))
        )),
        Line::from(format!(
            "Longest streak {} · current {}",
            plural(insights.longest_streak, "day"),
            plural(insights.current_streak, "day")
        )),
    ];
    if let Some(peak) = &insights.busiest_day {
        stats.push(Line::from(format!(
            "Peak day {} ({} commits)",
            safe(&peak.label),
            peak.count
        )));
    }
    let selection = repo.selection_labels();
    if !selection.is_empty() {
        stats.push(Line::from(format!(
            "Selection: {}",
            safe(&selection.join(" · "))
        )));
    }
    let [stats_area, spark_area, awards_area] = Layout::vertical([
        Constraint::Length(stats.len() as u16),
        Constraint::Length(4),
        Constraint::Min(2),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new(stats), stats_area);
    let monthly: Vec<u64> = data.activity.iter().map(|m| m.commits).collect();
    frame.render_widget(
        Sparkline::default()
            .block(section("Monthly commits"))
            .data(&monthly),
        spark_area,
    );
    let awards: Vec<String> = data
        .awards
        .iter()
        .take(3)
        .map(|a| format!("{}: {}", safe(&a.title), safe(&a.winner)))
        .collect();
    frame.render_widget(List::new(awards).block(section("Top awards")), awards_area);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::tui::state::tests::empty_data;
    use ratatui::{backend::TestBackend, Terminal};

    pub(crate) fn render(state: &AppState, data: &RepositoryAnalytics, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|frame| draw(frame, state, data)).unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(buffer.area.width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn overview_sanitizes_repository_text() {
        let mut data = empty_data();
        data.repository.name = "evil\x1b[31mname".into();
        data.awards.push(crate::model::Award {
            slug: "s".into(),
            title: "Top\x07award".into(),
            winner_id: "a@x".into(),
            winner: "Ann\x1b]0;x\x07".into(),
            metric: "commits".into(),
            value: "1".into(),
            explanation: "most".into(),
        });
        let text = render(&AppState::default(), &data, 80, 24);
        assert!(!text.chars().any(|c| c.is_control() && c != '\n'));
        assert!(text.contains("evil [31mname"));
        assert!(text.contains("Top award: Ann ]0;x "));
    }

    #[test]
    fn tiny_terminal_asks_for_resize() {
        let text = render(&AppState::default(), &empty_data(), 39, 11);
        assert!(text.contains("Resize"));
    }
}
