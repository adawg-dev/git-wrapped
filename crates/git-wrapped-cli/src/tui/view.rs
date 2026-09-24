use super::state::{sorted_contributors, sorted_files, AppState, Page, SortKey};
use crate::model::RepositoryAnalytics;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{
        Bar, BarChart, BarGroup, Block, Borders, List, Paragraph, Row, Sparkline, Table,
        TableState, Wrap,
    },
    Frame,
};

/// Below this width tables keep only their identifying and primary metric columns.
const WIDE: u16 = 70;

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
    let keys = match state.page {
        Page::Contributors if state.detail => "  Esc back",
        Page::Contributors => "  ↑↓ move  s sort  Enter detail",
        Page::Files | Page::Awards => "  ↑↓ move",
        _ => "",
    };
    frame.render_widget(Paragraph::new(format!("q quit  Tab/←→ page{keys}")), footer);
    match state.page {
        Page::Overview => overview(frame, body, data),
        Page::Contributors if state.detail => contributor_detail(frame, body, state, data),
        Page::Contributors => contributors(frame, body, state, data),
        Page::Activity => activity(frame, body, data),
        Page::Files => files(frame, body, state, data),
        Page::Awards => awards(frame, body, state, data),
        page => frame.render_widget(Paragraph::new(page.title()), body),
    }
}

fn table(
    frame: &mut Frame,
    area: Rect,
    selected: usize,
    header: Vec<String>,
    rows: Vec<Vec<String>>,
) {
    let widths: Vec<Constraint> = header
        .iter()
        .enumerate()
        .map(|(i, title)| {
            if i == 0 {
                Constraint::Fill(1)
            } else {
                let widest = rows
                    .iter()
                    .map(|row| row[i].chars().count())
                    .max()
                    .unwrap_or(0);
                Constraint::Length(widest.max(title.chars().count()) as u16)
            }
        })
        .collect();
    let empty = rows.is_empty();
    let table = Table::new(rows.into_iter().map(Row::new), widths)
        .header(Row::new(header).style(Style::new().add_modifier(Modifier::BOLD)))
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    if empty {
        let [head, note] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        frame.render_widget(table, head);
        frame.render_widget(Paragraph::new("No rows in this selection"), note);
    } else {
        let mut table_state = TableState::default().with_selected(Some(selected));
        frame.render_stateful_widget(table, area, &mut table_state);
    }
}

fn contributors(frame: &mut Frame, area: Rect, state: &AppState, data: &RepositoryAnalytics) {
    let keys = if area.width < WIDE {
        vec![state.sort]
    } else {
        vec![
            SortKey::Commits,
            SortKey::Additions,
            SortKey::Deletions,
            SortKey::Churn,
            SortKey::ActiveDays,
            SortKey::Files,
        ]
    };
    let header = std::iter::once("Name".to_string())
        .chain(keys.iter().map(|&key| {
            let marker = if key == state.sort { "▼" } else { "" };
            format!("{}{marker}", key.label())
        }))
        .collect();
    let rows = sorted_contributors(data, state.sort)
        .into_iter()
        .map(|c| {
            std::iter::once(safe(&c.name))
                .chain(keys.iter().map(|key| key.value(c).to_string()))
                .collect()
        })
        .collect();
    table(frame, area, state.selected, header, rows);
}

fn contributor_detail(frame: &mut Frame, area: Rect, state: &AppState, data: &RepositoryAnalytics) {
    let Some(c) = sorted_contributors(data, state.sort)
        .get(state.selected)
        .copied()
    else {
        frame.render_widget(Paragraph::new("No contributor selected"), area);
        return;
    };
    let lines = vec![
        Line::from(format!("{} <{}>", safe(&c.name), safe(&c.id))),
        Line::from(format!("Commits {} ({:.1}%)", c.commits, c.commit_percent)),
        Line::from(format!(
            "Additions {} · Deletions {}",
            c.additions, c.deletions
        )),
        Line::from(format!(
            "Churn {} · net {} historical lines",
            c.churn, c.net
        )),
        Line::from(format!(
            "Files {} · directories {}",
            c.files_touched, c.directories_touched
        )),
        Line::from(format!(
            "Active days {} · longest streak {}",
            c.active_days,
            plural(c.longest_streak, "day")
        )),
        Line::from(format!(
            "{} → {}",
            safe(&c.first_contribution),
            safe(&c.latest_contribution)
        )),
        Line::from(format!(
            "Commit size avg {:.1} · median {:.1} · max {}",
            c.average_commit_size, c.median_commit_size, c.largest_commit
        )),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
}

fn bars<'a>(labels: &[&'a str], values: &[u64], width: u16, title: &'a str) -> BarChart<'a> {
    let slots = values.len().max(1) as u16;
    let gap = u16::from(width / slots >= 3);
    let bar_width = (width / slots).saturating_sub(gap).max(1);
    let group: Vec<Bar> = labels
        .iter()
        .zip(values)
        .map(|(label, &value)| {
            let bar = Bar::default().value(value);
            if usize::from(bar_width) >= label.chars().count() {
                bar.label(Line::from(*label))
            } else {
                bar.text_value(String::new())
            }
        })
        .collect();
    BarChart::default()
        .block(section(title))
        .bar_width(bar_width)
        .bar_gap(gap)
        .data(BarGroup::default().bars(&group))
}

fn activity(frame: &mut Frame, area: Rect, data: &RepositoryAnalytics) {
    let [months, weekdays, hours] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Fill(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    let monthly: Vec<u64> = data.activity.iter().map(|m| m.commits).collect();
    let title = match (data.activity.first(), data.activity.last()) {
        (Some(first), Some(last)) => format!(
            "Monthly commits {} → {}",
            safe(&first.month),
            safe(&last.month)
        ),
        _ => "Monthly commits".into(),
    };
    frame.render_widget(
        Sparkline::default().block(section(&title)).data(&monthly),
        months,
    );
    let mut by_weekday = [0u64; 7];
    let mut by_hour = [0u64; 24];
    for c in &data.contributors {
        for (total, count) in by_weekday.iter_mut().zip(c.commits_by_weekday) {
            *total = total.saturating_add(count);
        }
        for (total, count) in by_hour.iter_mut().zip(c.commits_by_hour) {
            *total = total.saturating_add(count);
        }
    }
    let days = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    frame.render_widget(
        bars(&days, &by_weekday, weekdays.width, "Commits by weekday"),
        weekdays,
    );
    let hour_labels: Vec<String> = (0..24).map(|h| format!("{h:02}")).collect();
    let hour_labels: Vec<&str> = hour_labels.iter().map(String::as_str).collect();
    frame.render_widget(
        bars(&hour_labels, &by_hour, hours.width, "Commits by hour 00–23"),
        hours,
    );
}

fn files(frame: &mut Frame, area: Rect, state: &AppState, data: &RepositoryAnalytics) {
    let wide = area.width >= WIDE;
    let header = if wide {
        ["Path", "Revisions", "Churn▼", "Contributors", "Status"].as_slice()
    } else {
        ["Path", "Churn▼"].as_slice()
    };
    let rows = sorted_files(data)
        .into_iter()
        .map(|f| {
            let mut row = vec![safe(&f.display_path)];
            if wide {
                row.push(f.revisions.to_string());
            }
            row.push(f.churn.to_string());
            if wide {
                row.push(f.contributors.to_string());
                row.push(
                    if f.exists_at_head {
                        "current"
                    } else {
                        "historical"
                    }
                    .into(),
                );
            }
            row
        })
        .collect();
    table(
        frame,
        area,
        state.selected,
        header.iter().map(|h| h.to_string()).collect(),
        rows,
    );
}

fn awards(frame: &mut Frame, area: Rect, state: &AppState, data: &RepositoryAnalytics) {
    let [list, detail] = Layout::vertical([Constraint::Fill(1), Constraint::Length(4)]).areas(area);
    let wide = area.width >= WIDE;
    let header = if wide {
        vec!["Award".into(), "Winner".into(), "Value".into()]
    } else {
        vec!["Award".into(), "Winner".into()]
    };
    let rows = data
        .awards
        .iter()
        .map(|a| {
            let mut row = vec![safe(&a.title), safe(&a.winner)];
            if wide {
                row.push(safe(&a.value));
            }
            row
        })
        .collect();
    table(frame, list, state.selected, header, rows);
    if let Some(award) = data.awards.get(state.selected) {
        frame.render_widget(
            Paragraph::new(format!(
                "{} ({}): {}",
                safe(&award.value),
                safe(&award.metric),
                safe(&award.explanation)
            ))
            .block(section("Why"))
            .wrap(Wrap { trim: true }),
            detail,
        );
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
    use crate::tui::state::{
        tests::{empty_data, two_contributor_data},
        SortKey,
    };
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

    fn sample_data() -> RepositoryAnalytics {
        let mut data = two_contributor_data();
        data.contributors[1].name = "Bee\x1b[2J".into();
        data.activity.push(crate::model::Activity {
            month: "2024-01".into(),
            commits: 6,
            additions: 11,
            deletions: 0,
        });
        data.contributors[0].commits_by_weekday[0] = 5;
        data.contributors[0].commits_by_hour[9] = 5;
        data.files.push(crate::model::FileAnalytics {
            path_id: "old.rs".into(),
            display_path: "old\x07.rs".into(),
            revisions: 2,
            additions: 3,
            deletions: 3,
            churn: 6,
            contributors: 1,
            first_change: "2024-01-01".into(),
            latest_change: "2024-01-02".into(),
            exists_at_head: false,
            rename_from: vec![],
            longest_quiet_days: 1,
        });
        data.awards.push(crate::model::Award {
            slug: "commits".into(),
            title: "Commit Machine".into(),
            winner_id: "a@x".into(),
            winner: "a@x".into(),
            metric: "commits".into(),
            value: "5 commits".into(),
            explanation: "Most selected commits".into(),
        });
        data
    }

    #[test]
    fn every_page_renders_compact_and_wide_without_panicking() {
        for data in [empty_data(), sample_data()] {
            for page in Page::ALL {
                for detail in [false, true] {
                    let state = AppState {
                        page,
                        detail,
                        ..AppState::default()
                    };
                    for (w, h) in [(40, 12), (120, 40)] {
                        let text = render(&state, &data, w, h);
                        assert!(!text.contains("Resize"), "{page:?} {w}x{h}");
                        assert!(!text.chars().any(|c| c.is_control() && c != '\n'));
                    }
                }
            }
        }
    }

    #[test]
    fn contributor_table_shows_sort_and_all_columns_when_wide() {
        let state = AppState {
            page: Page::Contributors,
            sort: SortKey::Additions,
            ..AppState::default()
        };
        let wide = render(&state, &sample_data(), 120, 40);
        assert!(wide.contains("Additions▼") && wide.contains("Active days"));
        assert!(wide.contains("Bee [2J"));
        let narrow = render(&state, &sample_data(), 40, 12);
        assert!(narrow.contains("Additions▼") && !narrow.contains("Active days"));
        assert!(narrow.contains("s sort"));
    }

    #[test]
    fn contributor_detail_follows_selected_identity() {
        let state = AppState {
            page: Page::Contributors,
            selected: 1,
            selected_id: Some("b@x".into()),
            detail: true,
            ..AppState::default()
        };
        let text = render(&state, &sample_data(), 80, 24);
        assert!(text.contains("b@x") && text.contains("Esc back"));
        assert!(text.contains("Additions") && text.contains("10"));
    }

    #[test]
    fn activity_files_and_awards_show_their_metrics() {
        let data = sample_data();
        let page = |page| {
            render(
                &AppState {
                    page,
                    ..AppState::default()
                },
                &data,
                120,
                40,
            )
        };
        let activity = page(Page::Activity);
        assert!(activity.contains("Monthly commits") && activity.contains("Mon"));
        let files = page(Page::Files);
        assert!(files.contains("old .rs") && files.contains("historical"));
        let awards = page(Page::Awards);
        assert!(awards.contains("Commit Machine") && awards.contains("Most selected commits"));
    }

    #[test]
    fn tiny_terminal_asks_for_resize() {
        let text = render(&AppState::default(), &empty_data(), 39, 11);
        assert!(text.contains("Resize"));
    }
}
